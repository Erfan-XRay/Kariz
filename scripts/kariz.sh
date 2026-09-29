#!/usr/bin/env bash
# Kariz manager: installs Kariz and sets up and manages its tunnels on a Linux server
# with systemd. Each tunnel is /etc/kariz/<name>.toml run by the service kariz@<name>.
#
# One line, as root:
#   bash <(curl -fsSL https://raw.githubusercontent.com/Erfan-XRay/Kariz/main/scripts/kariz.sh)
#
# Without arguments it opens a menu. The same actions as commands (see `help`):
#   kariz-manager install [--version vX.Y.Z] [--binary PATH]
#   kariz-manager add NAME --role entry|exit --mode reverse|direct --transport T ...
#   kariz-manager list | status NAME | start|stop|restart NAME | logs NAME | speedtest NAME
#   kariz-manager edit NAME | remove NAME [--yes] | update | uninstall [--yes]
#
# The repository is private for now: set GITHUB_TOKEN to a token that can read it.
set -euo pipefail

REPO="Erfan-XRay/Kariz"
BIN=/usr/local/bin/kariz
MANAGER=/usr/local/bin/kariz-manager
CONF_DIR=/etc/kariz
UNIT=/etc/systemd/system/kariz@.service
RAW_URL="https://raw.githubusercontent.com/$REPO/main/scripts/kariz.sh"

# ---- Looks ----

if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
    C_RESET=$'\e[0m' C_BOLD=$'\e[1m' C_DIM=$'\e[2m'
    C_TEAL=$'\e[38;5;43m' C_AQUA=$'\e[38;5;86m' C_SAND=$'\e[38;5;179m'
    C_RED=$'\e[38;5;203m' C_YELLOW=$'\e[38;5;221m' C_GREEN=$'\e[38;5;78m'
else
    C_RESET='' C_BOLD='' C_DIM='' C_TEAL='' C_AQUA='' C_SAND='' C_RED='' C_YELLOW='' C_GREEN=''
fi

banner() {
    printf '\n%s' "$C_BOLD$C_TEAL"
    printf '  %s\n' ' _  __   _   ___  ___  ____' '| |/ /  /_\ | _ \|_ _||_  /'
    printf '%s' "$C_AQUA"
    printf '  %s\n' "| ' <  / _ \\|   / | |  / / " '|_|\_\/_/ \_\_|_\|___|/___|'
    printf '%s' "$C_RESET"
    printf '  %sKariz manager%s  %sdeveloped by%s %sErfanXRay%s\n\n' \
        "$C_BOLD$C_SAND" "$C_RESET" "$C_DIM" "$C_RESET" "$C_BOLD$C_SAND" "$C_RESET"
}

info() { printf '  %s●%s %s\n' "$C_TEAL" "$C_RESET" "$*"; }
ok() { printf '  %s✔%s %s\n' "$C_GREEN" "$C_RESET" "$*"; }
warn() { printf '  %s▲ %s%s\n' "$C_YELLOW" "$*" "$C_RESET" >&2; }
die() {
    printf '  %s✖ %s%s\n' "$C_RED" "$*" "$C_RESET" >&2
    exit 1
}

# Answers come from the terminal even when the script itself was piped in, or from the
# file in KARIZ_INPUT (the tests). Opened once, in the parent shell, so every question
# reads on from the same place. The braces keep the 2>/dev/null to the open itself: on a
# bare `exec` it would silence the whole script, the questions included.
IN_FD=""
open_input() {
    [[ -n "$IN_FD" ]] && return 0
    { exec {IN_FD}<"${KARIZ_INPUT:-/dev/tty}"; } 2>/dev/null || IN_FD=""
    [[ -n "$IN_FD" ]] ||
        die "This needs a terminal to ask questions; use the commands instead (kariz-manager help)."
}

# Asks a question and stores the answer in the variable named $1. Not run in $(...), so
# a missing answer ends the script instead of looping. $2: prompt, $3: default (may be
# empty).
ask() {
    local _var=$1 _prompt=$2 _default=${3:-} _answer
    open_input
    if [[ -n "$_default" ]]; then
        printf '  %s?%s %s %s[%s]%s: ' "$C_AQUA" "$C_RESET" "$_prompt" "$C_DIM" "$_default" "$C_RESET"
    else
        printf '  %s?%s %s: ' "$C_AQUA" "$C_RESET" "$_prompt"
    fi
    if ! IFS= read -r -u "$IN_FD" _answer; then
        echo
        die "No answer to: $_prompt"
    fi
    _answer=${_answer%$'\r'}
    printf -v "$_var" '%s' "${_answer:-$_default}"
}

# Asks to pick one of the words in $3 (space separated), into the variable named $1.
choose() {
    local _var=$1 _prompt=$2 _options=$3 _default=${4:-} _pick _o
    while true; do
        ask _pick "$_prompt (${_options// / | })" "$_default"
        for _o in $_options; do
            if [[ "$_pick" == "$_o" ]]; then
                printf -v "$_var" '%s' "$_pick"
                return
            fi
        done
        warn "Pick one of: $_options"
    done
}

confirm() {
    local _yes
    ask _yes "$1 (y/n)" "${2:-n}"
    [[ "$_yes" == y* || "$_yes" == Y* ]]
}

# ---- Checks ----

need_root() {
    [[ $EUID -eq 0 ]] || die "Run as root (sudo)."
}

need_systemd() {
    command -v systemctl >/dev/null || die "This needs a Linux server with systemd."
}

need_kariz() {
    [[ -x "$BIN" ]] || die "Kariz is not installed yet: run '$(basename "$0") install' first."
}

valid_name() {
    [[ "$1" =~ ^[a-zA-Z0-9][a-zA-Z0-9_-]{0,31}$ ]] ||
        die "Tunnel names use letters, digits, - and _ (up to 32)."
}

conf_of() { printf '%s/%s.toml' "$CONF_DIR" "$1"; }

need_tunnel() {
    valid_name "$1"
    [[ -f "$(conf_of "$1")" ]] || die "No tunnel named '$1' (see: list)."
}

# ---- Download and install ----

arch() {
    case "$(uname -m)" in
        x86_64 | amd64) echo x86_64 ;;
        aarch64 | arm64) echo aarch64 ;;
        armv7l | armv7*) echo armv7 ;;
        *) die "No Kariz build for this CPU ($(uname -m))." ;;
    esac
}

# curl or wget, with the token for a private repository. $1: URL, $2: output file or
# - for stdout, $3: extra header (optional).
fetch() {
    local url=$1 out=$2 header=${3:-}
    local auth=()
    [[ -n "${GITHUB_TOKEN:-}" ]] && auth=(-H "Authorization: Bearer $GITHUB_TOKEN")
    if command -v curl >/dev/null; then
        local extra=()
        [[ -n "$header" ]] && extra=(-H "$header")
        curl -fsSL --retry 3 "${auth[@]}" "${extra[@]}" -o "$out" "$url"
    elif command -v wget >/dev/null; then
        local extra=()
        [[ -n "${GITHUB_TOKEN:-}" ]] && extra+=(--header="Authorization: Bearer $GITHUB_TOKEN")
        [[ -n "$header" ]] && extra+=(--header="$header")
        wget -q "${extra[@]}" -O "$out" "$url"
    else
        die "Needs curl or wget."
    fi
}

latest_version() {
    fetch "https://api.github.com/repos/$REPO/releases/latest" - |
        sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1
}

# Downloads release asset $2 of version $1 to file $3. With a token it goes through the
# API (works for private repositories), otherwise the public download link.
download_asset() {
    local version=$1 name=$2 out=$3
    if [[ -z "${GITHUB_TOKEN:-}" ]]; then
        fetch "https://github.com/$REPO/releases/download/$version/$name" "$out"
        return
    fi
    local api
    api=$(fetch "https://api.github.com/repos/$REPO/releases/tags/$version" - |
        awk -v name="$name" '
            /"url": *"https:\/\/api.github.com\/.*\/releases\/assets\// {
                gsub(/.*"url": *"|".*/, ""); url = $0
            }
            $0 ~ "\"name\": *\"" name "\"" { print url; exit }
        ')
    [[ -n "$api" ]] || die "Release $version has no $name."
    fetch "$api" "$out" "Accept: application/octet-stream"
}

install_unit() {
    cat >"$UNIT" <<'EOF'
[Unit]
Description=Kariz tunnel %i
Documentation=https://github.com/Erfan-XRay/Kariz
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/kariz run -c /etc/kariz/%i.toml
Restart=always
RestartSec=2
LimitNOFILE=1048576
AmbientCapabilities=CAP_NET_BIND_SERVICE CAP_NET_RAW
NoNewPrivileges=true

[Install]
WantedBy=multi-user.target
EOF
    systemctl daemon-reload
}

# Puts this script in place as kariz-manager: from its own file when it is one, else
# from the repository (when it was run through `bash <(curl ...)`).
install_manager() {
    local self=${BASH_SOURCE[0]}
    if [[ -f "$self" && "$(realpath "$self")" != "$MANAGER" ]]; then
        install -m 0755 "$self" "$MANAGER"
    elif [[ ! -f "$MANAGER" ]] || [[ "$(realpath "$self")" != "$MANAGER" ]]; then
        local tmp
        tmp=$(mktemp)
        if fetch "$RAW_URL" "$tmp"; then
            install -m 0755 "$tmp" "$MANAGER"
        else
            warn "Could not install the kariz-manager command (download failed)."
        fi
        rm -f "$tmp"
    fi
}

cmd_install() {
    need_root
    need_systemd
    local version="" binary=""
    while [[ $# -gt 0 ]]; do
        case $1 in
            --version) version=$2 && shift 2 ;;
            --binary) binary=$2 && shift 2 ;;
            *) die "install: unknown option $1" ;;
        esac
    done
    if [[ -n "$binary" ]]; then
        [[ -x "$binary" ]] || die "No executable at $binary."
        install -m 0755 "$binary" "$BIN"
    else
        local a tmp
        a=$(arch)
        [[ -n "$version" ]] || version=$(latest_version)
        [[ -n "$version" ]] || die "Could not find the latest release (private repository? set GITHUB_TOKEN)."
        info "Downloading Kariz $version for $a"
        local name="kariz-$version-$a-linux"
        tmp=$(mktemp -d)
        download_asset "$version" "$name.tar.gz" "$tmp/$name.tar.gz"
        download_asset "$version" "$name.tar.gz.sha256" "$tmp/$name.tar.gz.sha256"
        (cd "$tmp" && sha256sum -c --quiet "$name.tar.gz.sha256") ||
            die "Checksum mismatch: the download is damaged."
        tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
        install -m 0755 "$tmp/$name/kariz" "$BIN"
        rm -rf "$tmp"
    fi
    mkdir -p "$CONF_DIR"
    chmod 700 "$CONF_DIR"
    install_unit
    install_manager
    ok "Installed: $("$BIN" --version)"
    info "Manage it any time with: ${C_BOLD}kariz-manager${C_RESET}"
}

cmd_update() {
    need_root
    need_kariz
    local before
    before=$("$BIN" --version)
    cmd_install "$@"
    local restarted=0
    for unit in $(systemctl list-units --type=service --state=active --plain --no-legend 'kariz@*' | awk '{print $1}'); do
        systemctl restart "$unit" && restarted=$((restarted + 1))
    done
    ok "$before -> $("$BIN" --version); $restarted running tunnel(s) restarted"
}

cmd_uninstall() {
    need_root
    local yes=${1:-}
    if [[ "$yes" != --yes ]]; then
        confirm "Stop all tunnels and remove Kariz?" || return 0
    fi
    for unit in $(systemctl list-unit-files --plain --no-legend 'kariz@*' | awk '{print $1}') \
        $(systemctl list-units --all --plain --no-legend 'kariz@*' | awk '{print $1}'); do
        [[ "$unit" == *@.service ]] && continue
        systemctl disable --now "$unit" 2>/dev/null || true
    done
    rm -f "$UNIT" "$BIN"
    systemctl daemon-reload
    if [[ "$yes" == --yes ]] || confirm "Also delete the tunnel configs in $CONF_DIR (tokens included)?"; then
        rm -rf "$CONF_DIR"
    fi
    ok "Kariz removed."
    rm -f "$MANAGER"
}

# ---- Tunnels ----

# The server's own address, as a default for the other side to dial.
own_ip() {
    ip -4 route get 1.1.1.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -n 1
}

# "443=127.0.0.1:443/udp" -> listen, target, protocol. A bare port listens everywhere.
parse_forward() {
    local spec=$1 listen target proto=tcp
    [[ "$spec" == *=* ]] || die "Forward rules look like LISTEN=TARGET[/tcp|udp|tcp+udp], e.g. 443=127.0.0.1:443"
    listen=${spec%%=*}
    target=${spec#*=}
    if [[ "$target" == */* ]]; then
        proto=${target##*/}
        target=${target%/*}
    fi
    [[ "$listen" == *:* ]] || listen="0.0.0.0:$listen"
    [[ "$proto" =~ ^(tcp|udp|tcp\+udp)$ ]] || die "Forward protocol must be tcp, udp or tcp+udp."
    printf '%s\t%s\t%s' "$listen" "$target" "$proto"
}

# Writes /etc/kariz/NAME.toml from the settings in the variables below, checks it with
# `kariz check`, and starts the tunnel.
write_tunnel() {
    local conf
    conf=$(conf_of "$T_NAME")
    local tmp="$conf.new"
    {
        echo "# Kariz tunnel '$T_NAME', written by kariz-manager on $(date -u '+%Y-%m-%d %H:%M UTC')."
        echo "# Edit with: kariz-manager edit $T_NAME"
        echo "role = \"$T_ROLE\""
        echo "mode = \"$T_MODE\""
        echo "profile = \"$T_PROFILE\""
        echo
        echo "[tunnel]"
        echo "transport = \"$T_TRANSPORT\""
        [[ -n "$T_LISTEN" ]] && echo "listen = \"$T_LISTEN\""
        [[ -n "$T_REMOTE" ]] && echo "remote = \"$T_REMOTE\""
        echo "token = \"$T_TOKEN\""
        if [[ "$T_TRANSPORT" == ws || "$T_TRANSPORT" == wss ]]; then
            echo
            echo "[tunnel.ws]"
            echo "path = \"$T_WS_PATH\""
            [[ -n "$T_LISTEN" ]] || echo "early_data = true"
        fi
        if [[ "$T_TRANSPORT" == wss ]]; then
            echo
            echo "[tunnel.tls]"
            if [[ -n "$T_LISTEN" ]]; then
                echo "cert = \"$CONF_DIR/$T_NAME.crt\""
                echo "key = \"$CONF_DIR/$T_NAME.key\""
            else
                echo "pin_sha256 = \"$T_PIN\""
            fi
        fi
        local f
        for f in "${T_FORWARDS[@]}"; do
            local listen target proto
            IFS=$'\t' read -r listen target proto <<<"$(parse_forward "$f")"
            echo
            echo "[[forward]]"
            echo "listen = \"$listen\""
            echo "target = \"$target\""
            echo "protocol = \"$proto\""
        done
    } >"$tmp"
    chmod 600 "$tmp"
    local check
    if ! check=$("$BIN" check -c "$tmp" 2>&1); then
        rm -f "$tmp"
        printf '%s\n' "$check" >&2
        die "Kariz rejected this configuration (above); nothing was changed."
    fi
    mv "$tmp" "$conf"
    systemctl enable --now "kariz@$T_NAME" >/dev/null 2>&1 ||
        die "The tunnel did not start: kariz-manager logs $T_NAME"
    ok "Tunnel '$T_NAME' is running ($T_ROLE, $T_MODE, $T_TRANSPORT)."
}

# A self-signed certificate for a wss listener; prints its pin.
make_certificate() {
    command -v openssl >/dev/null || die "wss needs openssl to make a certificate."
    local crt="$CONF_DIR/$T_NAME.crt" key="$CONF_DIR/$T_NAME.key"
    local err
    err=$(openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 3650 \
        -subj "/CN=${T_WS_HOST:-localhost}" -keyout "$key" -out "$crt" 2>&1) ||
        die "openssl could not make a certificate: $err"
    chmod 600 "$key"
    "$BIN" pin "$crt"
}

# The command that sets up the matching side on the other server.
peer_command() {
    local role=exit
    [[ "$T_ROLE" == exit ]] && role=entry
    local cmd="kariz-manager add $T_NAME --role $role --mode $T_MODE --transport $T_TRANSPORT"
    cmd+=" --profile $T_PROFILE --token $T_TOKEN"
    if [[ -n "$T_LISTEN" ]]; then
        cmd+=" --remote ${T_PUBLIC:-SERVER_IP}:${T_LISTEN##*:}"
    else
        cmd+=" --listen 0.0.0.0:${T_REMOTE##*:}"
    fi
    [[ "$T_TRANSPORT" == ws || "$T_TRANSPORT" == wss ]] && cmd+=" --ws-path $T_WS_PATH"
    [[ -n "${T_PIN_OUT:-}" ]] && cmd+=" --pin $T_PIN_OUT"
    if [[ "$role" == entry ]]; then
        cmd+=" --forward LISTEN_PORT=TARGET:PORT"
    fi
    printf '%s' "$cmd"
}

show_peer_command() {
    printf '\n  %sOn the other server, run:%s\n\n' "$C_BOLD" "$C_RESET"
    printf '    %s%s%s\n\n' "$C_SAND" "$(peer_command)" "$C_RESET"
    if [[ "$T_ROLE" == exit ]]; then
        info "Replace LISTEN_PORT=TARGET:PORT with the entry's forward rules (--forward can repeat)."
    fi
    [[ -n "$T_LISTEN" ]] && info "Open ${T_LISTEN##*:}/${T_PROTO_HINT} in this server's firewall."
    return 0
}

reset_tunnel_vars() {
    T_NAME="" T_ROLE="" T_MODE="" T_TRANSPORT="" T_PROFILE=balanced T_LISTEN="" T_REMOTE=""
    T_TOKEN="" T_WS_PATH="" T_WS_HOST="" T_PIN="" T_PIN_OUT="" T_PUBLIC="" T_FORWARDS=()
}

# Fills in what the tunnel's settings imply: which side listens, a token, a ws path.
finish_tunnel_vars() {
    local listens=false
    if [[ "$T_ROLE" == entry && "$T_MODE" == reverse ]] || [[ "$T_ROLE" == exit && "$T_MODE" == direct ]]; then
        listens=true
    fi
    if $listens; then
        [[ -n "$T_LISTEN" ]] || die "This side listens: give --listen ADDRESS:PORT."
        T_REMOTE=""
    else
        [[ -n "$T_REMOTE" ]] || die "This side dials: give --remote ADDRESS:PORT."
        T_LISTEN=""
    fi
    [[ -n "$T_TOKEN" ]] || T_TOKEN=$("$BIN" token)
    if [[ "$T_TRANSPORT" == ws || "$T_TRANSPORT" == wss ]]; then
        [[ -n "$T_WS_PATH" ]] || T_WS_PATH="/$(head -c 6 /dev/urandom | od -An -tx1 | tr -d ' \n')"
    fi
    [[ "$T_ROLE" == exit && ${#T_FORWARDS[@]} -gt 0 ]] && die "Forward rules belong on the entry side."
    [[ "$T_ROLE" == entry && ${#T_FORWARDS[@]} -eq 0 ]] && die "The entry side needs at least one --forward rule."
    case $T_TRANSPORT in
        quic | kcp) T_PROTO_HINT=udp ;;
        *) T_PROTO_HINT=tcp ;;
    esac
    return 0
}

cmd_add() {
    need_root
    need_kariz
    reset_tunnel_vars
    T_NAME=${1:-}
    [[ -n "$T_NAME" ]] || die "add: give the tunnel a name."
    shift
    valid_name "$T_NAME"
    [[ -f "$(conf_of "$T_NAME")" ]] && die "A tunnel named '$T_NAME' exists already."
    while [[ $# -gt 0 ]]; do
        case $1 in
            --role) T_ROLE=$2 ;;
            --mode) T_MODE=$2 ;;
            --transport) T_TRANSPORT=$2 ;;
            --profile) T_PROFILE=$2 ;;
            --listen) T_LISTEN=$2 ;;
            --remote) T_REMOTE=$2 ;;
            --token) T_TOKEN=$2 ;;
            --forward) T_FORWARDS+=("$2") ;;
            --ws-path) T_WS_PATH=$2 ;;
            --pin) T_PIN=$2 ;;
            --public-ip) T_PUBLIC=$2 ;;
            *) die "add: unknown option $1" ;;
        esac
        shift 2
    done
    [[ "$T_ROLE" =~ ^(entry|exit)$ ]] || die "add: --role entry or exit."
    [[ "$T_MODE" =~ ^(reverse|direct)$ ]] || die "add: --mode reverse or direct."
    [[ "$T_TRANSPORT" =~ ^(tcp|tcpmux|ws|wss|quic|kcp)$ ]] || die "add: --transport tcp, tcpmux, ws, wss, quic or kcp."
    finish_tunnel_vars
    if [[ "$T_TRANSPORT" == wss ]]; then
        if [[ -n "$T_LISTEN" ]]; then
            T_PIN_OUT=$(make_certificate)
        else
            [[ -n "$T_PIN" ]] || die "A wss dialer needs --pin (printed when the listening side was added)."
        fi
    fi
    [[ -n "$T_PUBLIC" ]] || T_PUBLIC=$(own_ip)
    write_tunnel
    show_peer_command
}

# The interactive version of `add`.
wizard_add() {
    need_root
    need_kariz
    open_input
    reset_tunnel_vars
    printf '\n  %sNew tunnel%s\n\n' "$C_BOLD" "$C_RESET"
    info "Entry = the server users connect to. Exit = the server that reaches the targets."
    info "Reverse = the exit dials the entry. Direct = the entry dials the exit."
    echo
    while true; do
        ask T_NAME "Name for this tunnel" main
        if [[ ! "$T_NAME" =~ ^[a-zA-Z0-9][a-zA-Z0-9_-]{0,31}$ ]]; then
            warn "Use letters, digits, - and _."
        elif [[ -f "$(conf_of "$T_NAME")" ]]; then
            warn "A tunnel named '$T_NAME' exists already."
        else
            break
        fi
    done
    choose T_ROLE "This server is the" "entry exit" entry
    choose T_MODE "Mode" "reverse direct" reverse
    info "tcpmux: most uses. wss: looks like HTTPS, works through CDNs. kcp: lossy links, games. quic: over UDP."
    choose T_TRANSPORT "Transport" "tcp tcpmux ws wss quic kcp" tcpmux
    choose T_PROFILE "Profile" "balanced ultraspeed gaming" balanced
    local listens=false
    if [[ "$T_ROLE" == entry && "$T_MODE" == reverse ]] || [[ "$T_ROLE" == exit && "$T_MODE" == direct ]]; then
        listens=true
    fi
    if $listens; then
        local port
        while true; do
            ask port "Port the other server connects to" 3080
            [[ "$port" =~ ^[0-9]+$ ]] && ((port >= 1 && port <= 65535)) && break
            warn "A port is a number from 1 to 65535."
        done
        T_LISTEN="0.0.0.0:$port"
        ask T_PUBLIC "This server's public IP (for the other side)" "$(own_ip)"
    else
        while [[ -z "$T_REMOTE" ]]; do
            ask T_REMOTE "Other server's address (IP:PORT)"
        done
        [[ "$T_REMOTE" == *:* ]] || T_REMOTE="$T_REMOTE:3080"
    fi
    if confirm "Generate a new token? (no: paste the other server's)" y; then
        T_TOKEN=$("$BIN" token)
    else
        while [[ -z "$T_TOKEN" ]]; do
            ask T_TOKEN "Token"
        done
    fi
    if [[ "$T_TRANSPORT" == ws || "$T_TRANSPORT" == wss ]]; then
        ask T_WS_PATH "WebSocket path (same on both sides)" "/$(head -c 6 /dev/urandom | od -An -tx1 | tr -d ' \n')"
    fi
    if [[ "$T_TRANSPORT" == wss && $listens == false ]]; then
        while [[ -z "$T_PIN" ]]; do
            ask T_PIN "Certificate pin (printed by the listening side)"
        done
    fi
    if [[ "$T_ROLE" == entry ]]; then
        info "Forward rules: LISTEN=TARGET[/tcp|udp|tcp+udp]. The target is dialed from the exit."
        info "Example: 443=127.0.0.1:443   or   51820=127.0.0.1:51820/udp"
        while true; do
            local rule
            ask rule "Forward rule (empty to finish)"
            [[ -z "$rule" ]] && [[ ${#T_FORWARDS[@]} -gt 0 ]] && break
            [[ -z "$rule" ]] && {
                warn "Add at least one rule."
                continue
            }
            (parse_forward "$rule" >/dev/null) && T_FORWARDS+=("$rule")
        done
    fi
    finish_tunnel_vars
    if [[ "$T_TRANSPORT" == wss && $listens == true ]]; then
        T_PIN_OUT=$(make_certificate)
    fi
    echo
    write_tunnel
    show_peer_command
}

cmd_list() {
    shopt -s nullglob
    local files=("$CONF_DIR"/*.toml)
    if [[ ${#files[@]} -eq 0 ]]; then
        info "No tunnels yet. Add one with: kariz-manager add (or the menu)."
        return
    fi
    printf '\n  %s%-16s %-6s %-8s %-8s %-10s %s%s\n' "$C_DIM" NAME ROLE MODE TRANSPORT STATE ADDRESS "$C_RESET"
    local f
    for f in "${files[@]}"; do
        local name role mode transport addr state color
        name=$(basename "$f" .toml)
        role=$(sed -n 's/^role *= *"\(.*\)"/\1/p' "$f")
        mode=$(sed -n 's/^mode *= *"\(.*\)"/\1/p' "$f")
        transport=$(sed -n 's/^transport *= *"\(.*\)"/\1/p' "$f")
        addr=$(sed -n 's/^\(listen\|remote\) *= *"\(.*\)"/\1 \2/p' "$f" | head -n 1)
        state=$(systemctl is-active "kariz@$name" 2>/dev/null || true)
        case $state in
            active) color=$C_GREEN ;;
            failed) color=$C_RED ;;
            *) color=$C_YELLOW ;;
        esac
        printf '  %-16s %-6s %-8s %-8s %s%-10s%s %s\n' "$name" "$role" "$mode" "${transport:-tcp}" \
            "$color" "${state:-stopped}" "$C_RESET" "$addr"
    done
    echo
}

cmd_service() {
    local action=$1 name=${2:-}
    [[ -n "$name" ]] || die "$action: which tunnel?"
    need_tunnel "$name"
    case $action in
        start) systemctl enable --now "kariz@$name" && ok "Started '$name'." ;;
        stop) systemctl disable --now "kariz@$name" && ok "Stopped '$name' (it stays off after a reboot)." ;;
        restart) systemctl restart "kariz@$name" && ok "Restarted '$name'." ;;
        status) systemctl status "kariz@$name" --no-pager ;;
        logs)
            info "Ctrl+C to stop following the log."
            journalctl -u "kariz@$name" -n 100 -f
            ;;
    esac
}

cmd_edit() {
    need_root
    local name=${1:-}
    [[ -n "$name" ]] || die "edit: which tunnel?"
    need_tunnel "$name"
    local conf tmp
    conf=$(conf_of "$name")
    tmp=$(mktemp --suffix=.toml)
    cp "$conf" "$tmp"
    local editor=${EDITOR:-}
    if [[ -z "$editor" ]]; then
        editor="vi"
        command -v nano >/dev/null && editor="nano"
    fi
    # The editor needs the terminal, also when this script itself was piped in.
    if [[ -t 0 ]]; then
        $editor "$tmp"
    else
        open_input
        $editor "$tmp" <&"$IN_FD"
    fi
    local check
    if ! check=$("$BIN" check -c "$tmp" 2>&1); then
        printf '%s\n' "$check" >&2
        rm -f "$tmp"
        die "Kariz rejected the edit (above); the tunnel was not changed."
    fi
    install -m 600 "$tmp" "$conf"
    rm -f "$tmp"
    systemctl restart "kariz@$name"
    ok "Saved and restarted '$name'."
}

cmd_remove() {
    need_root
    local name=${1:-} yes=${2:-}
    [[ -n "$name" ]] || die "remove: which tunnel?"
    need_tunnel "$name"
    if [[ "$yes" != --yes ]]; then
        confirm "Remove tunnel '$name' (its config and token too)?" || return 0
    fi
    systemctl disable --now "kariz@$name" >/dev/null 2>&1 || true
    rm -f "$(conf_of "$name")" "$CONF_DIR/$name.crt" "$CONF_DIR/$name.key"
    ok "Removed '$name'."
}

# ---- Menu ----

# The speed test runs on the entry side, through the running tunnel's own sessions.
cmd_speedtest() {
    need_root
    need_kariz
    local name=${1:-}
    [[ -n "$name" ]] || die "speedtest: which tunnel?"
    shift
    need_tunnel "$name"
    local conf role
    conf=$(conf_of "$name")
    role=$(sed -n 's/^role *= *"\(.*\)"/\1/p' "$conf")
    [[ "$role" == entry ]] ||
        die "Run the speed test on the entry server: 'kariz-manager speedtest $name' there."
    systemctl is-active --quiet "kariz@$name" ||
        die "The tunnel '$name' is not running (kariz-manager start $name)."
    "$BIN" speedtest -c "$conf" "$@"
}

# Lists the tunnels and asks for one, by number or name, into the variable named $1.
pick_tunnel() {
    local _var=$1 _names=() _f _pick _i
    shopt -s nullglob
    for _f in "$CONF_DIR"/*.toml; do
        _names+=("$(basename "$_f" .toml)")
    done
    [[ ${#_names[@]} -gt 0 ]] || die "No tunnels yet (option 2 adds one)."
    echo
    for _i in "${!_names[@]}"; do
        printf '   %s%d%s) %s\n' "$C_TEAL" $((_i + 1)) "$C_RESET" "${_names[$_i]}"
    done
    echo
    while true; do
        ask _pick "Tunnel (number or name)" "$([[ ${#_names[@]} -eq 1 ]] && echo 1)"
        if [[ "$_pick" =~ ^[0-9]+$ ]] && ((_pick >= 1 && _pick <= ${#_names[@]})); then
            _pick=${_names[$((_pick - 1))]}
        fi
        for _f in "${_names[@]}"; do
            if [[ "$_pick" == "$_f" ]]; then
                printf -v "$_var" '%s' "$_pick"
                return
            fi
        done
        warn "No tunnel '$_pick'."
    done
}

menu() {
    need_root
    need_systemd
    open_input
    # Ctrl+C (to leave a log) ends the action and comes back here; the actions run in
    # subshells, which take the default action for it.
    trap 'echo' INT
    while true; do
        banner
        if [[ -x "$BIN" ]]; then
            info "Installed: $("$BIN" --version)"
        else
            warn "Kariz is not installed yet (option 1)."
        fi
        cat <<EOF

   ${C_TEAL}1${C_RESET}) Install or update Kariz
   ${C_TEAL}2${C_RESET}) New tunnel
   ${C_TEAL}3${C_RESET}) List tunnels
   ${C_TEAL}4${C_RESET}) Start / stop / restart a tunnel
   ${C_TEAL}5${C_RESET}) Logs of a tunnel
   ${C_TEAL}6${C_RESET}) Speed test a tunnel (on the entry side)
   ${C_TEAL}7${C_RESET}) Edit a tunnel
   ${C_TEAL}8${C_RESET}) Remove a tunnel
   ${C_TEAL}9${C_RESET}) Uninstall Kariz
   ${C_TEAL}0${C_RESET}) Exit

EOF
        local choice
        ask choice "Choose"
        # Each action runs in a subshell, so an error returns to the menu.
        case $choice in
            1) (if [[ -x "$BIN" ]]; then cmd_update; else cmd_install; fi) || true ;;
            2) (wizard_add) || true ;;
            3) cmd_list ;;
            4) (
                pick_tunnel name
                choose action "Action" "start stop restart status" restart
                cmd_service "$action" "$name"
            ) || true ;;
            5) (pick_tunnel name && cmd_service logs "$name") || true ;;
            6) (pick_tunnel name && cmd_speedtest "$name") || true ;;
            7) (pick_tunnel name && cmd_edit "$name") || true ;;
            8) (pick_tunnel name && cmd_remove "$name") || true ;;
            9) (cmd_uninstall) || true ;;
            0 | q) exit 0 ;;
            *) warn "Choose 0-9." ;;
        esac
        printf '\n  %sEnter to go back to the menu%s' "$C_DIM" "$C_RESET"
        read -r -u "$IN_FD" _ || exit 0
    done
}

usage() {
    banner
    cat <<EOF
  Usage: kariz-manager [command]      (no command: the menu)

  install [--version vX.Y.Z] [--binary PATH]   install Kariz (latest release by default)
  update  [--version vX.Y.Z]                   update Kariz and restart running tunnels
  add NAME --role entry|exit --mode reverse|direct --transport T [options]
      --listen ADDR:PORT | --remote ADDR:PORT   the listening or the dialing side
      --token TOKEN        default: a new one    --profile balanced|ultraspeed|gaming
      --forward LISTEN=TARGET[/tcp|udp|tcp+udp] entry only, can repeat
      --ws-path /PATH      ws / wss              --pin HEX   wss dialer
  list                                         all tunnels and their state
  start | stop | restart | status | logs NAME
  speedtest NAME [--seconds N] [--streams N] [--no-udp]   speed and latency (entry side)
  edit NAME                                    edit, check and restart
  remove NAME [--yes]
  uninstall [--yes]                            also deletes the configs with --yes

  The repository is private for now: set GITHUB_TOKEN to download releases.
EOF
}

main() {
    case ${1:-} in
        "") menu ;;
        install) shift && cmd_install "$@" ;;
        update) shift && cmd_update "$@" ;;
        uninstall) cmd_uninstall "${2:-}" ;;
        add) shift && cmd_add "$@" ;;
        list) cmd_list ;;
        start | stop | restart | status | logs) need_root && cmd_service "$1" "${2:-}" ;;
        speedtest) shift && cmd_speedtest "$@" ;;
        edit) cmd_edit "${2:-}" ;;
        remove) cmd_remove "${2:-}" "${3:-}" ;;
        help | -h | --help) usage ;;
        *) usage && exit 1 ;;
    esac
}

# Not when sourced (the tests load the functions). Piped into bash, there is no source
# file at all.
if [[ "${BASH_SOURCE[0]:-$0}" == "$0" ]]; then
    main "$@"
fi
