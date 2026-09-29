#!/usr/bin/env bash
# Kariz manager: installs Kariz and sets up and manages its tunnels on a Linux server
# with systemd. Each tunnel is /etc/kariz/<name>.toml run by the service kariz@<name>.
#
# One line, as root:
#   bash <(curl -fsSL https://raw.githubusercontent.com/Erfan-XRay/Kariz/main/scripts/kariz.sh)
#
# Without arguments it opens a menu. The same actions as commands (see `help`):
#   kariz-manager install [--version vX.Y.Z] [--binary PATH]
#   kariz-manager add NAME --role entry|exit --mode reverse|direct --transport T --ports 443,...
#   kariz-manager list | status NAME | start|stop|restart NAME | logs NAME | speedtest NAME
#   kariz-manager edit NAME | remove NAME [--yes] | update | uninstall [--yes]
#   kariz-manager panel install | link | password | status | logs | uninstall
#   kariz-manager --agent CODE        connect this server to a panel (its join code)
#
# The repository is private for now: set GITHUB_TOKEN to a token that can read it.
set -euo pipefail

REPO="Erfan-XRay/Kariz"
BIN=/usr/local/bin/kariz
MANAGER=/usr/local/bin/kariz-manager
CONF_DIR=/etc/kariz
UNIT=/etc/systemd/system/kariz@.service
# The web panel (docs/panel.md) and its agent.
PANEL_BIN=/usr/local/bin/kariz-panel
PANEL_DIR=/etc/kariz-panel
PANEL_DATA=/var/lib/kariz-panel
PANEL_CONF=$PANEL_DIR/panel.toml
AGENT_CONF=$PANEL_DIR/agent.toml
PANEL_UNIT=/etc/systemd/system/kariz-panel.service
AGENT_UNIT=/etc/systemd/system/kariz-agent.service
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

# Asks to pick one option from a numbered list; the answer is its number or its word.
# Into the variable named $1. $2: the question, $3: the default word, then the options
# as "word" or "word|description".
choose() {
    local _var=$1 _prompt=$2 _default=$3 _pick _o _n=0 _def=""
    shift 3
    local _words=()
    printf '  %s%s%s\n' "$C_BOLD" "$_prompt" "$C_RESET"
    for _o in "$@"; do
        _n=$((_n + 1))
        _words+=("${_o%%|*}")
        [[ "${_o%%|*}" == "$_default" ]] && _def=$_n
        if [[ "$_o" == *"|"* ]]; then
            printf '    %s%d)%s %-10s %s%s%s\n' "$C_TEAL" "$_n" "$C_RESET" "${_o%%|*}" "$C_DIM" "${_o#*|}" "$C_RESET"
        else
            printf '    %s%d)%s %s\n' "$C_TEAL" "$_n" "$C_RESET" "$_o"
        fi
    done
    while true; do
        ask _pick "Choose" "$_def"
        if [[ "$_pick" =~ ^[0-9]+$ ]] && ((_pick >= 1 && _pick <= _n)); then
            _pick=${_words[$((_pick - 1))]}
        fi
        for _o in "${_words[@]}"; do
            if [[ "$_pick" == "$_o" ]]; then
                printf -v "$_var" '%s' "$_pick"
                return
            fi
        done
        warn "Type a number from 1 to $_n."
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

# The key releases are signed with (docs/PHASE14.md). Only the release workflow has the
# private half. It is checked with openssl (1.1.1 or later, or 3).
RELEASE_KEY_PEM='-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAWGxcNhmewk/DsktXLy2UgJ5ZOLS0fktw7D2/obNvjTs=
-----END PUBLIC KEY-----'
# A user can pin a key of their own (a fork, or a test): KARIZ_RELEASE_KEY_PEM.
RELEASE_KEY_PEM=${KARIZ_RELEASE_KEY_PEM:-$RELEASE_KEY_PEM}

# Checks the signature of a downloaded archive: $1 the directory with the archive, its
# .sha256 and its .sig, $2 the archive's name, $3 the version. Releases before 0.11 were not
# signed: they are accepted with a warning; from 0.11 on a missing or wrong signature stops
# the installation.
verify_signature() {
    local dir=$1 name=$2 version=$3
    local file="$dir/$name.tar.gz"
    if [[ ! -s "$file.sig" ]]; then
        if version_lt "${version#v}" "0.11.0"; then
            warn "This release is not signed (releases before 0.11 were not): only its checksum was checked."
            return 0
        fi
        die "This release has no signature: it will not be installed."
    fi
    if ! command -v openssl >/dev/null; then
        die "Checking the signature needs openssl (apt install openssl). Nothing was installed."
    fi
    printf '%s\n' "$RELEASE_KEY_PEM" >"$dir/release-key.pem"
    openssl pkeyutl -verify -pubin -inkey "$dir/release-key.pem" -rawin \
        -in "$file.sha256" -sigfile "$file.sig" >/dev/null 2>&1 ||
        die "The signature does not match: this download is not from the Kariz release key. Nothing was installed."
    # The signed checksum file must name this archive, so a signed archive of another CPU
    # cannot be put in its place.
    [[ "$(awk '{print $2}' "$file.sha256" | sed 's/^\*//')" == "$name.tar.gz" ]] ||
        die "The signed checksum is for another file. Nothing was installed."
    ok "The release signature is good."
}

# 0 if version $1 is lower than $2 (plain numbers, x.y.z; a suffix after a dash is ignored).
version_lt() {
    local a=${1%%-*} b=${2%%-*}
    [[ "$a" != "$b" && "$(printf '%s\n%s\n' "$a" "$b" | sort -V | head -n1)" == "$a" ]]
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

install_panel_units() {
    cat >"$PANEL_UNIT" <<'EOF'
[Unit]
Description=Kariz web panel
Documentation=https://github.com/Erfan-XRay/Kariz
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/kariz-panel serve -c /etc/kariz-panel/panel.toml
Restart=always
RestartSec=2
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF
    cat >"$AGENT_UNIT" <<'EOF'
[Unit]
Description=Kariz agent (connects this server to a Kariz panel)
Documentation=https://github.com/Erfan-XRay/Kariz
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/kariz-panel agent -c /etc/kariz-panel/agent.toml
Restart=always
RestartSec=2
NoNewPrivileges=true
PrivateTmp=true

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
    local version="" binary="" panel_binary=""
    while [[ $# -gt 0 ]]; do
        case $1 in
            --version) version=$2 && shift 2 ;;
            --binary) binary=$2 && shift 2 ;;
            --panel-binary) panel_binary=$2 && shift 2 ;;
            *) die "install: unknown option $1" ;;
        esac
    done
    if [[ -n "$panel_binary" ]]; then
        [[ -x "$panel_binary" ]] || die "No executable at $panel_binary."
        install -m 0755 "$panel_binary" "$PANEL_BIN"
    fi
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
        # The signature is optional to download (releases before 0.11 have none).
        (download_asset "$version" "$name.tar.gz.sig" "$tmp/$name.tar.gz.sig") 2>/dev/null ||
            rm -f "$tmp/$name.tar.gz.sig"
        (cd "$tmp" && sha256sum -c --quiet "$name.tar.gz.sha256") ||
            die "Checksum mismatch: the download is damaged."
        verify_signature "$tmp" "$name" "$version"
        tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
        install -m 0755 "$tmp/$name/kariz" "$BIN"
        # Releases from 0.8 carry the web panel and its agent in the same archive.
        if [[ -f "$tmp/$name/kariz-panel" ]]; then
            install -m 0755 "$tmp/$name/kariz-panel" "$PANEL_BIN"
        fi
        rm -rf "$tmp"
    fi
    mkdir -p "$CONF_DIR"
    chmod 700 "$CONF_DIR"
    install_unit
    if [[ -x "$PANEL_BIN" ]]; then
        install_panel_units
    fi
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
    local svc
    for svc in kariz-panel kariz-agent; do
        if systemctl is-active --quiet "$svc"; then
            systemctl restart "$svc"
            info "Restarted $svc."
        fi
    done
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

# ---- Addresses and ports ----

# At most this many forwarded ports per tunnel: a range becomes one rule per port.
MAX_FORWARDS=1000

# The server's own addresses, as defaults for the other side to dial (empty without).
own_ip4() {
    { ip -4 route get 1.1.1.1 2>/dev/null || true; } | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -n 1
}

own_ip6() {
    { ip -6 route get 2606:4700:4700::1111 2>/dev/null || true; } | sed -n 's/.* src \([0-9a-fA-F:]*\).*/\1/p' | head -n 1
}

# Whether this server has IPv6. Listening on [::] then takes IPv4 as well.
ipv6_on() {
    [[ -e /proc/net/if_inet6 ]] && [[ "$(cat /proc/sys/net/ipv6/conf/all/disable_ipv6 2>/dev/null || echo 0)" == 0 ]]
}

# The address to listen on: [::] (IPv4 and IPv6) where the server has IPv6.
default_bind() {
    if ipv6_on; then echo "::"; else echo "0.0.0.0"; fi
}

valid_port() {
    [[ "$1" =~ ^[0-9]{1,5}$ ]] && ((10#$1 >= 1 && 10#$1 <= 65535))
}

# HOST and PORT as one address; an IPv6 host goes in brackets.
host_port() {
    if [[ "$1" == *:* && "$1" != \[* ]]; then
        printf '[%s]:%s' "$1" "$2"
    else
        printf '%s:%s' "$1" "$2"
    fi
}

# Splits an address as people type it into ADDR_HOST and ADDR_PORT (empty when not
# given): 1.2.3.4, 1.2.3.4:3080, 2001:db8::1, [2001:db8::1]:3080, example.com:443.
# Returns 1 on something else.
split_addr() {
    local a=${1// /}
    local bracketed='^\[([0-9a-fA-F:.]+)\](:([0-9]+))?$' named='^([a-zA-Z0-9.-]+)(:([0-9]+))?$'
    ADDR_HOST="" ADDR_PORT=""
    if [[ "$a" =~ $bracketed ]]; then
        ADDR_HOST=${BASH_REMATCH[1]} ADDR_PORT=${BASH_REMATCH[3]}
    elif [[ "$a" =~ ^[0-9a-fA-F:.]+$ && "$a" == *:*:* ]]; then
        ADDR_HOST=$a
    elif [[ "$a" =~ $named ]]; then
        ADDR_HOST=${BASH_REMATCH[1]} ADDR_PORT=${BASH_REMATCH[3]}
        # a bare number is a mistyped port or address, not a host
        [[ ! "$ADDR_HOST" =~ ^[0-9]+$ ]] || return 1
    else
        return 1
    fi
    [[ -z "$ADDR_PORT" ]] || valid_port "$ADDR_PORT"
}

# Ports in use on this server, as " 22/tcp 53/udp ... ".
BUSY_PORTS=" "
load_busy_ports() {
    command -v ss >/dev/null || return 0
    BUSY_PORTS=" $({
        { ss -Hltn || true; } | awk '{ n = split($4, a, ":"); print a[n] "/tcp" }'
        { ss -Hlun || true; } | awk '{ n = split($4, a, ":"); print a[n] "/udp" }'
    } 2>/dev/null | sort -u | tr '\n' ' ') "
}

port_busy() {
    [[ "$BUSY_PORTS" == *" $1/$2 "* ]]
}

proto_words() {
    case $1 in
        tcp) echo tcp ;;
        udp) echo udp ;;
        *) echo tcp udp ;;
    esac
}

# "443" or "1000-2000" into the variables named $2 and $3 (first and last port).
port_span() {
    local _a _b
    if [[ "$1" =~ ^([0-9]+)-([0-9]+)$ ]]; then
        _a=${BASH_REMATCH[1]} _b=${BASH_REMATCH[2]}
    elif [[ "$1" =~ ^[0-9]+$ ]]; then
        _a=$1 _b=$1
    else
        return 1
    fi
    if ! valid_port "$_a" || ! valid_port "$_b" || ((10#$_a > 10#$_b)); then
        return 1
    fi
    printf -v "$2" '%d' $((10#$_a))
    printf -v "$3" '%d' $((10#$_b))
}

# Turns a port list into forward rules and adds them to T_FORWARDS:
#   443, 8443        the same port on both sides
#   8080-8090        a range
#   2053=53          users connect to 2053, the exit dials 53 (8443:443 works too)
#   3000-3005=4000-4005   a range onto another range of the same size
#   5000-5010=443    many ports onto one
# $1: the list, $2: target host (as the exit sees it), $3: tcp, udp or tcp+udp,
# $4: the host to listen on. Says what is wrong and returns 1 on a bad list, adding
# nothing.
expand_ports() {
    local spec=${1// /} proto=$3 bind target
    local items item from to lo hi tlo thi p t k rule new=() seen=" "
    bind=$(host_port "$4" 0) && bind=${bind%:0}
    target=$(host_port "$2" 0) && target=${target%:0}
    IFS=, read -ra items <<<"$spec"
    for rule in "${T_FORWARDS[@]}"; do
        local listen=${rule%%=*} rp=tcp
        [[ "$rule" == */* ]] && rp=${rule##*/}
        for k in $(proto_words "$rp"); do seen+="${listen##*:}/$k "; done
    done
    for item in "${items[@]}"; do
        [[ -n "$item" ]] || continue
        item=${item//:/=}
        from=${item%%=*} to=${item#*=}
        if ! port_span "$from" lo hi || ! port_span "$to" tlo thi; then
            warn "'$item' is not a port (443), a range (8080-8090) or a mapping (2053=53)."
            return 1
        fi
        if ((thi - tlo != hi - lo && tlo != thi)); then
            warn "'$item': a range maps onto a range of the same size, or onto one port."
            return 1
        fi
        for ((p = lo; p <= hi; p++)); do
            t=$((tlo == thi ? tlo : tlo + p - lo))
            for k in $(proto_words "$proto"); do
                if [[ "$seen" == *" $p/$k "* ]]; then
                    warn "Port $p/$k is in the list twice."
                    return 1
                fi
                if port_busy "$p" "$k"; then
                    warn "Port $p/$k is already in use on this server."
                    return 1
                fi
                seen+="$p/$k "
            done
            new+=("$bind:$p=$target:$t/$proto")
            if ((${#T_FORWARDS[@]} + ${#new[@]} > MAX_FORWARDS)); then
                warn "At most $MAX_FORWARDS ports per tunnel."
                return 1
            fi
        done
    done
    if [[ ${#new[@]} -eq 0 ]]; then
        warn "Give at least one port."
        return 1
    fi
    T_FORWARDS+=("${new[@]}")
}

# ---- Tunnels ----

# "443=127.0.0.1:443/udp" -> listen, target, protocol. A bare port listens on T_BIND.
parse_forward() {
    local spec=$1 listen target proto=tcp
    [[ "$spec" == *=* ]] || die "Forward rules look like LISTEN=TARGET[/tcp|udp|tcp+udp], e.g. 443=127.0.0.1:443"
    listen=${spec%%=*}
    target=${spec#*=}
    if [[ "$target" == */* ]]; then
        proto=${target##*/}
        target=${target%/*}
    fi
    [[ "$listen" == *:* ]] || listen=$(host_port "${T_BIND:-0.0.0.0}" "$listen")
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
        local f listen target proto
        for f in "${T_FORWARDS[@]}"; do
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
    sleep 1
    systemctl is-active --quiet "kariz@$T_NAME" ||
        die "The tunnel stopped right after starting: kariz-manager logs $T_NAME"
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
        cmd+=" --remote $(host_port "${T_PUBLIC:-SERVER_IP}" "${T_LISTEN##*:}")"
    else
        cmd+=" --listen ${T_REMOTE##*:}"
    fi
    [[ "$T_TRANSPORT" == ws || "$T_TRANSPORT" == wss ]] && cmd+=" --ws-path $T_WS_PATH"
    [[ -n "${T_PIN_OUT:-}" ]] && cmd+=" --pin $T_PIN_OUT"
    [[ "$role" == entry ]] && cmd+=" --ports PORTS"
    printf '%s' "$cmd"
}

show_peer_command() {
    printf '\n  %sOn the other server, run:%s\n\n' "$C_BOLD" "$C_RESET"
    printf '    %s%s%s\n\n' "$C_SAND" "$(peer_command)" "$C_RESET"
    if [[ "$T_ROLE" == exit ]]; then
        info "Replace PORTS with the ports to forward, e.g. 443,8080-8090 (add --protocol udp for UDP)."
    fi
    [[ -n "$T_LISTEN" ]] && info "Open ${T_LISTEN##*:}/${T_PROTO_HINT} in this server's firewall."
    [[ ${#T_FORWARDS[@]} -gt 0 ]] && info "Open the forwarded ports in this server's firewall too."
    return 0
}

reset_tunnel_vars() {
    T_NAME="" T_ROLE="" T_MODE="" T_TRANSPORT="" T_PROFILE=balanced T_LISTEN="" T_REMOTE=""
    T_TOKEN="" T_WS_PATH="" T_WS_HOST="" T_PIN="" T_PIN_OUT="" T_PUBLIC="" T_FORWARDS=()
    T_BIND=$(default_bind)
}

# Whether this side listens (else it dials): the entry in reverse mode, the exit in direct.
tunnel_listens() {
    [[ "$T_ROLE" == entry && "$T_MODE" == reverse ]] || [[ "$T_ROLE" == exit && "$T_MODE" == direct ]]
}

# Fills in what the tunnel's settings imply: which side listens, a token, a ws path.
finish_tunnel_vars() {
    if tunnel_listens; then
        [[ -n "$T_LISTEN" ]] || die "This side listens: give --listen PORT (or ADDRESS:PORT)."
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
    [[ "$T_ROLE" == entry && ${#T_FORWARDS[@]} -eq 0 ]] && die "The entry side needs ports to forward (--ports)."
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
    local ports=() proto=tcp to=127.0.0.1
    while [[ $# -gt 0 ]]; do
        case $1 in
            --ipv4-only)
                T_BIND=0.0.0.0
                shift
                continue
                ;;
        esac
        [[ $# -ge 2 ]] || die "add: $1 needs a value."
        case $1 in
            --role) T_ROLE=$2 ;;
            --mode) T_MODE=$2 ;;
            --transport) T_TRANSPORT=$2 ;;
            --profile) T_PROFILE=$2 ;;
            --listen) T_LISTEN=$2 ;;
            --remote) T_REMOTE=$2 ;;
            --token) T_TOKEN=$2 ;;
            --forward) T_FORWARDS+=("$2") ;;
            --ports) ports+=("$2") ;;
            --protocol) proto=$2 ;;
            --to) to=$2 ;;
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
    [[ "$proto" =~ ^(tcp|udp|tcp\+udp)$ ]] || die "add: --protocol tcp, udp or tcp+udp."
    # --listen 3080 listens on every address; --remote takes IPv6 with or without brackets.
    if [[ -n "$T_LISTEN" ]] && valid_port "$T_LISTEN"; then
        T_LISTEN=$(host_port "$T_BIND" "$T_LISTEN")
    fi
    if [[ -n "$T_REMOTE" ]]; then
        split_addr "$T_REMOTE" || die "add: --remote $T_REMOTE is not an address."
        T_REMOTE=$(host_port "$ADDR_HOST" "${ADDR_PORT:-3080}")
    fi
    load_busy_ports
    local spec
    for spec in "${ports[@]}"; do
        if ! split_addr "$to" || [[ -n "$ADDR_PORT" ]]; then
            die "add: --to takes a host without a port."
        fi
        expand_ports "$spec" "$ADDR_HOST" "$proto" "$T_BIND" || die "add: bad --ports $spec"
    done
    finish_tunnel_vars
    if [[ "$T_TRANSPORT" == wss ]]; then
        if [[ -n "$T_LISTEN" ]]; then
            T_PIN_OUT=$(make_certificate)
        else
            [[ -n "$T_PIN" ]] || die "A wss dialer needs --pin (printed when the listening side was added)."
        fi
    fi
    [[ -n "$T_PUBLIC" ]] || T_PUBLIC=$(own_ip4)
    [[ -n "$T_PUBLIC" ]] || T_PUBLIC=$(own_ip6)
    write_tunnel
    show_peer_command
}

# A heading for a step of the wizard.
step() {
    printf '\n  %s%s[%s/%s]%s %s%s%s\n' "$C_BOLD" "$C_TEAL" "$1" "$2" "$C_RESET" "$C_BOLD" "$3" "$C_RESET"
}

# The interactive version of `add`.
wizard_add() {
    need_root
    need_kariz
    open_input
    reset_tunnel_vars
    load_busy_ports
    local steps=6
    printf '\n  %sNew tunnel%s  %s(Ctrl+C cancels and goes back to the menu)%s\n' \
        "$C_BOLD" "$C_RESET" "$C_DIM" "$C_RESET"

    step 1 $steps "Name"
    while true; do
        ask T_NAME "Name for this tunnel" main
        if [[ ! "$T_NAME" =~ ^[a-zA-Z0-9][a-zA-Z0-9_-]{0,31}$ ]]; then
            warn "Use letters, digits, - and _ (up to 32)."
        elif [[ -f "$(conf_of "$T_NAME")" ]]; then
            warn "A tunnel named '$T_NAME' exists already."
        else
            break
        fi
    done

    step 2 $steps "This server's side"
    choose T_ROLE "This server is the" entry \
        "entry|users connect to this server" \
        "exit|this server reaches the targets (the open internet)"
    choose T_MODE "Who connects to whom" reverse \
        "reverse|the exit connects to the entry" \
        "direct|the entry connects to the exit"

    step 3 $steps "Transport and profile"
    choose T_TRANSPORT "Transport" tcpmux \
        "tcpmux|TCP with mux: the best choice for most uses" \
        "tcp|plain TCP, one connection per user" \
        "ws|WebSocket: through HTTP proxies and CDNs" \
        "wss|WebSocket over TLS: looks like HTTPS, works through CDNs" \
        "kcp|over UDP: lossy links, games" \
        "quic|QUIC over UDP"
    choose T_PROFILE "Profile" balanced \
        "balanced|good for everything" \
        "ultraspeed|the most throughput, more memory" \
        "gaming|the lowest latency, for games and calls"

    step 4 $steps "Connection between the servers"
    local port family
    if tunnel_listens; then
        info "This server waits for the other one to connect."
        while true; do
            ask port "Port for the tunnel" 3080
            if ! valid_port "$port"; then
                warn "A port is a number from 1 to 65535."
            elif port_busy "$port" tcp || port_busy "$port" udp; then
                warn "Port $port is already in use on this server."
            else
                break
            fi
        done
        if ipv6_on; then
            choose family "Accept the other server over" both \
                "both|IPv4 and IPv6" "ipv4|IPv4 only"
            T_BIND=0.0.0.0
            [[ "$family" == both ]] && T_BIND="::"
        fi
        T_LISTEN=$(host_port "$T_BIND" "$port")
        local v4 v6 options=()
        v4=$(own_ip4)
        v6=$(own_ip6)
        [[ -n "$v4" ]] && options+=("$v4|IPv4 of this server")
        [[ -n "$v6" && "$T_BIND" == "::" ]] && options+=("$v6|IPv6 of this server")
        options+=("other|another address or a domain")
        choose T_PUBLIC "Address the other server connects to" "${v4:-other}" "${options[@]}"
        while [[ "$T_PUBLIC" == other ]] || ! split_addr "$T_PUBLIC" || [[ -n "$ADDR_PORT" ]]; do
            ask T_PUBLIC "This server's address (IPv4, IPv6 or domain, no port)"
        done
    else
        info "This server connects to the other one."
        while true; do
            ask T_REMOTE "Other server's address (IPv4, IPv6 or domain; :PORT optional)"
            if split_addr "$T_REMOTE"; then
                break
            fi
            warn "Examples: 1.2.3.4   1.2.3.4:3080   2001:db8::1   [2001:db8::1]:3080   example.com"
        done
        local host=$ADDR_HOST
        port=$ADDR_PORT
        while [[ -z "$port" ]] || ! valid_port "$port"; do
            ask port "Port of the tunnel on the other server" 3080
        done
        T_REMOTE=$(host_port "$host" "$port")
    fi

    step 5 $steps "Security"
    local how
    choose how "Token (the shared secret; the same on both servers)" new \
        "new|make a new one (on the first server you set up)" \
        "paste|paste the other server's"
    if [[ "$how" == new ]]; then
        T_TOKEN=$("$BIN" token)
    else
        while [[ -z "$T_TOKEN" ]]; do
            ask T_TOKEN "Token"
        done
    fi
    if [[ "$T_TRANSPORT" == ws || "$T_TRANSPORT" == wss ]]; then
        ask T_WS_PATH "WebSocket path (same on both sides)" "/$(head -c 6 /dev/urandom | od -An -tx1 | tr -d ' \n')"
    fi
    if [[ "$T_TRANSPORT" == wss ]] && ! tunnel_listens; then
        while [[ -z "$T_PIN" ]]; do
            ask T_PIN "Certificate pin (printed by the listening side)"
        done
    fi

    step 6 $steps "Ports to forward"
    if [[ "$T_ROLE" == entry ]]; then
        info "Users connect to these ports on this server; the exit passes them on."
        info "Write ports with commas: ${C_BOLD}443${C_RESET}   ${C_BOLD}443,8443${C_RESET}   ${C_BOLD}8080-8090${C_RESET} (range)   ${C_BOLD}2053=53${C_RESET} (2053 here, 53 on the target)"
        if ! tunnel_listens && ipv6_on; then
            choose family "Users connect over" both "both|IPv4 and IPv6" "ipv4|IPv4 only"
            T_BIND=0.0.0.0
            [[ "$family" == both ]] && T_BIND="::"
        fi
        while true; do
            local spec proto target
            choose proto "Protocol" tcp \
                "tcp|TCP: web, V2Ray/Xray, most apps" \
                "udp|UDP: games, WireGuard, DNS" \
                "tcp+udp|both"
            while true; do
                ask target "Target host, as the exit server reaches it" 127.0.0.1
                split_addr "$target" && [[ -z "$ADDR_PORT" ]] && break
                warn "A host without a port: 127.0.0.1, ::1, 10.0.0.5 or a domain."
            done
            target=$ADDR_HOST
            while true; do
                ask spec "Ports"
                local before=${#T_FORWARDS[@]}
                if expand_ports "$spec" "$target" "$proto" "$T_BIND"; then
                    ok "$((${#T_FORWARDS[@]} - before)) port(s) added: $spec -> $target ($proto)"
                    break
                fi
            done
            confirm "Add more ports (another protocol or target)?" n || break
        done
    else
        info "Nothing to do here: the entry server chooses the ports."
    fi

    finish_tunnel_vars
    summary
    if ! confirm "Create this tunnel?" y; then
        info "Cancelled; nothing was changed."
        return 0
    fi
    if [[ "$T_TRANSPORT" == wss ]] && tunnel_listens; then
        T_PIN_OUT=$(make_certificate)
    fi
    echo
    write_tunnel
    show_peer_command
}

summary_row() {
    printf '    %s%-12s%s %s\n' "$C_DIM" "$1" "$C_RESET" "$2"
}

summary() {
    printf '\n  %sSummary%s\n' "$C_BOLD" "$C_RESET"
    summary_row name "$T_NAME"
    summary_row side "$T_ROLE, $T_MODE"
    summary_row transport "$T_TRANSPORT, profile $T_PROFILE"
    if [[ -n "$T_LISTEN" ]]; then
        local both=""
        [[ "$T_LISTEN" == "[::]:"* ]] && both=" (IPv4 and IPv6)"
        summary_row listens "$T_LISTEN$both"
        summary_row "peer dials" "$(host_port "$T_PUBLIC" "${T_LISTEN##*:}")"
    else
        summary_row connects "$T_REMOTE"
    fi
    if [[ ${#T_FORWARDS[@]} -gt 0 ]]; then
        summary_row forwards "${#T_FORWARDS[@]} port(s)"
        local f shown=0
        for f in "${T_FORWARDS[@]}"; do
            if ((shown == 5)); then
                summary_row "" "..."
                break
            fi
            summary_row "" "${f%%=*} -> ${f#*=}"
            shown=$((shown + 1))
        done
    fi
    echo
}

cmd_list() {
    shopt -s nullglob
    local files=("$CONF_DIR"/*.toml)
    if [[ ${#files[@]} -eq 0 ]]; then
        info "No tunnels yet. Add one with: kariz-manager add (or the menu)."
        return
    fi
    printf '\n  %s%-16s %-6s %-8s %-9s %-6s %-10s %s%s\n' "$C_DIM" NAME ROLE MODE TRANSPORT PORTS STATE ADDRESS "$C_RESET"
    local f
    for f in "${files[@]}"; do
        local name role mode transport addr state color ports
        name=$(basename "$f" .toml)
        role=$(sed -n 's/^role *= *"\(.*\)"/\1/p' "$f")
        mode=$(sed -n 's/^mode *= *"\(.*\)"/\1/p' "$f")
        transport=$(sed -n 's/^transport *= *"\(.*\)"/\1/p' "$f")
        addr=$(sed -n 's/^\(listen\|remote\) *= *"\(.*\)"/\1 \2/p' "$f" | head -n 1)
        ports=$(grep -c '^\[\[forward\]\]' "$f" || true)
        [[ "$ports" == 0 ]] && ports=-
        state=$(systemctl is-active "kariz@$name" 2>/dev/null || true)
        case $state in
            active) color=$C_GREEN ;;
            failed) color=$C_RED ;;
            *) color=$C_YELLOW ;;
        esac
        printf '  %-16s %-6s %-8s %-9s %-6s %s%-10s%s %s\n' "$name" "$role" "$mode" "${transport:-tcp}" \
            "$ports" "$color" "${state:-stopped}" "$C_RESET" "$addr"
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
        status) cmd_status "$name" ;;
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

# Whether this Kariz has `kariz status` (v0.7 and later).
has_status() { "$BIN" status --help >/dev/null 2>&1; }

# A tunnel's live status (connection, round trip, traffic per port) from `kariz status`,
# or systemd's view when it is not running. Without a name: every tunnel.
cmd_status() {
    need_root
    need_kariz
    local name=${1:-}
    if [[ -z "$name" ]]; then
        cmd_status_all
        return
    fi
    shift
    need_tunnel "$name"
    if systemctl is-active --quiet "kariz@$name" && has_status; then
        "$BIN" status -c "$(conf_of "$name")" "$@"
    else
        systemctl status "kariz@$name" --no-pager
    fi
}

cmd_status_all() {
    shopt -s nullglob
    local f name any=0
    for f in "$CONF_DIR"/*.toml; do
        any=1
        name=$(basename "$f" .toml)
        if ! systemctl is-active --quiet "kariz@$name"; then
            printf '\n  %s▸%s tunnel %s · %sstopped%s\n' "$C_TEAL" "$C_RESET" "$name" "$C_YELLOW" "$C_RESET"
        elif has_status; then
            "$BIN" status -c "$f" || true
        else
            printf '\n  %s▸%s tunnel %s · running (update Kariz to see its status)\n' "$C_TEAL" "$C_RESET" "$name"
        fi
    done
    ((any)) || info "No tunnels yet (option 2 adds one)."
}

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
        printf '    %s%d)%s %s\n' "$C_TEAL" $((_i + 1)) "$C_RESET" "${_names[$_i]}"
    done
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

# ---- The web panel and its agent ----

# The panel's address, certificate and ports, from its settings.
panel_setting() { sed -n "s/^$1 = \"\(.*\)\"/\1/p" "$PANEL_CONF" | head -n 1; }

# This server's address as another machine would use it: --host, else its IPv4, else IPv6.
panel_host() {
    local host=${1:-}
    [[ -n "$host" ]] || host=$(own_ip4)
    [[ -n "$host" ]] || host=$(own_ip6)
    [[ -n "$host" ]] || host="<this-server>"
    printf '%s' "$host"
}

panel_show() {
    local host=$1 fingerprint=${2:-}
    local listen path agent
    listen=$(panel_setting listen)
    path=$(panel_setting path)
    agent=$(panel_setting agent_listen)
    echo
    printf '  %s address     %s https://%s:%s/%s/\n' "$C_TEAL" "$C_RESET" "$host" "${listen##*:}" "$path"
    if [[ -n "$fingerprint" ]]; then
        printf '  %s certificate %s SHA-256 %s\n' "$C_TEAL" "$C_RESET" "$fingerprint"
        printf '  %s             %s (self-signed: the browser warns; compare this fingerprint)\n' "$C_DIM" "$C_RESET"
    fi
    if [[ -n "$agent" ]]; then
        printf '  %s agents      %s port %s (open it in the firewall for the servers you add)\n' "$C_TEAL" "$C_RESET" "${agent##*:}"
    fi
    printf '  %s sign in     %s ' "$C_TEAL" "$C_RESET"
    "$PANEL_BIN" login-link -c "$PANEL_CONF" --host "$host" 2>/dev/null
    printf '  %s             %s (works once, for 60 minutes; make another with: kariz-manager panel link)\n' "$C_DIM" "$C_RESET"
}

panel_install() {
    need_root
    need_systemd
    local port="" host="" version=()
    while [[ $# -gt 0 ]]; do
        case $1 in
            --port) port=$2 && shift 2 ;;
            --host) host=$2 && shift 2 ;;
            --version) version=(--version "$2") && shift 2 ;;
            *) die "panel install: unknown option $1" ;;
        esac
    done
    if [[ ! -x "$PANEL_BIN" ]]; then
        cmd_install "${version[@]}"
    fi
    [[ -x "$PANEL_BIN" ]] ||
        die "This Kariz release has no web panel. Install 0.8 or later: kariz-manager update"
    local init_args=(-c "$PANEL_CONF" --data-dir "$PANEL_DATA")
    if [[ -n "$port" ]]; then
        init_args+=(--port "$port")
    fi
    mkdir -p "$PANEL_DIR" "$PANEL_DATA"
    chmod 700 "$PANEL_DIR" "$PANEL_DATA"
    local out fingerprint
    out=$("$PANEL_BIN" init "${init_args[@]}")
    fingerprint=$(printf '%s\n' "$out" | sed -n 's/.*SHA-256 \([0-9a-f]*\).*/\1/p')
    install_panel_units
    systemctl enable --now kariz-panel
    sleep 2
    systemctl is-active --quiet kariz-panel ||
        die "The panel did not start: journalctl -u kariz-panel -n 50"
    ok "The web panel is running."
    panel_show "$(panel_host "$host")" "$fingerprint"
}

cmd_panel() {
    local action=${1:-}
    shift || true
    case $action in
        install) panel_install "$@" ;;
        link)
            need_root
            [[ -f "$PANEL_CONF" ]] || die "The panel is not installed: kariz-manager panel install"
            local host=""
            if [[ ${1:-} == --host ]]; then
                host=${2:-}
            fi
            "$PANEL_BIN" login-link -c "$PANEL_CONF" --host "$(panel_host "$host")"
            ;;
        password)
            need_root
            [[ -f "$PANEL_CONF" ]] || die "The panel is not installed: kariz-manager panel install"
            "$PANEL_BIN" reset-password -c "$PANEL_CONF" "$@"
            ;;
        status)
            need_systemd
            systemctl status kariz-panel --no-pager || true
            if [[ -f "$PANEL_CONF" ]]; then
                panel_show "$(panel_host "")"
            fi
            ;;
        logs) journalctl -u kariz-panel -n 100 -f ;;
        uninstall) panel_uninstall "$@" ;;
        *) die "panel: install [--port N] [--host H] | link | password [--stdin] | status | logs | uninstall [--yes]" ;;
    esac
}

panel_uninstall() {
    need_root
    local yes=${1:-}
    if [[ "$yes" != --yes ]]; then
        confirm "Stop the web panel and remove it?" || return 0
    fi
    systemctl disable --now kariz-panel 2>/dev/null || true
    rm -f "$PANEL_UNIT"
    systemctl daemon-reload
    if [[ ! -f "$AGENT_CONF" ]]; then
        rm -f "$PANEL_BIN"
    fi
    if [[ "$yes" == --yes ]] || confirm "Also delete the panel's database, certificate and settings (the servers it knows)?"; then
        rm -rf "$PANEL_DATA" "$PANEL_CONF"
    fi
    ok "The web panel is removed."
}

# Connects this server to a panel: `kariz-manager --agent CODE`.
agent_join() {
    need_root
    need_systemd
    local code=${1:-} version=()
    shift || true
    while [[ $# -gt 0 ]]; do
        case $1 in
            --version) version=(--version "$2") && shift 2 ;;
            *) die "agent: unknown option $1" ;;
        esac
    done
    [[ "$code" == kz1_* ]] || die "That is not a join code (it starts with kz1_). Copy it whole from the panel: Servers, Add server."
    if [[ ! -x "$PANEL_BIN" ]]; then
        cmd_install "${version[@]}"
    fi
    [[ -x "$PANEL_BIN" ]] ||
        die "This Kariz release has no agent. Install 0.8 or later: kariz-manager update, then --agent CODE"
    mkdir -p "$PANEL_DIR"
    chmod 700 "$PANEL_DIR"
    "$PANEL_BIN" agent --join "$code" --no-run -c "$AGENT_CONF"
    install_panel_units
    systemctl enable --now kariz-agent
    sleep 2
    systemctl is-active --quiet kariz-agent ||
        die "The agent did not start: journalctl -u kariz-agent -n 50"
    ok "This server is connected: it shows up in the panel under Servers."
    info "Follow it with: kariz-manager agent logs"
}

cmd_agent() {
    local action=${1:-}
    case $action in
        kz1_*) agent_join "$@" ;;
        join) shift && agent_join "$@" ;;
        status)
            need_systemd
            systemctl status kariz-agent --no-pager
            ;;
        logs) journalctl -u kariz-agent -n 100 -f ;;
        remove)
            need_root
            systemctl disable --now kariz-agent 2>/dev/null || true
            rm -f "$AGENT_UNIT" "$AGENT_CONF"
            systemctl daemon-reload
            ok "The agent is removed. Remove the server in the panel too (Servers, Remove)."
            ;;
        *) die "agent: CODE | join CODE | status | logs | remove" ;;
    esac
}

# The private network links of this server, read only: the kz- interfaces that the panel's
# agent made (docs/networks.md). The panel is where they are made and removed.
cmd_net() {
    command -v ip >/dev/null || die "net: this needs the 'ip' command."
    case ${1:-list} in
        list)
            local rows
            rows=$(ip -br addr show 2>/dev/null | awk '$1 ~ /^kz-/')
            if [[ -z "$rows" ]]; then
                info "No private network links on this server."
            else
                printf '%s\n' "$rows"
            fi
            ;;
        status)
            local name=${2:-}
            [[ "$name" =~ ^kz-[a-z0-9]{1,8}$ ]] || die "net status: give the link's name (kariz-manager net list)."
            ip link show "$name" >/dev/null 2>&1 || die "There is no link '$name' on this server."
            ip -d -s link show "$name"
            ip -br addr show "$name"
            ;;
        *) die "net: list | status NAME" ;;
    esac
}

# The menu's web panel entry.
menu_panel() {
    local action code
    choose action "Web panel" install \
        "install|install the web panel on this server" \
        "link|make a one-time login link" \
        "password|set a new admin password" \
        "status|its address and state" \
        "agent|connect this server to a panel (with a join code)" \
        "uninstall|remove the web panel"
    case $action in
        install) panel_install ;;
        agent)
            ask code "Join code (starts with kz1_)"
            agent_join "$code"
            ;;
        *) cmd_panel "$action" ;;
    esac
}

# Ctrl+C at the menu's own question leaves the manager. During an action (which runs in
# a subshell, and so ends on it) it comes back to the menu at once.
MENU_BUSY=0 INTERRUPTED=0
on_interrupt() {
    echo
    if ((MENU_BUSY)); then
        INTERRUPTED=1
    else
        exit 0
    fi
}

# "Kariz v0.6.0 · 3 tunnels, 2 running" for the top of the menu.
menu_status() {
    if [[ ! -x "$BIN" ]]; then
        warn "Kariz is not installed yet: choose 1."
        return
    fi
    shopt -s nullglob
    local files=("$CONF_DIR"/*.toml) running
    running=$(systemctl list-units --type=service --state=active --plain --no-legend 'kariz@*' 2>/dev/null | wc -l)
    info "$("$BIN" --version)  ${C_DIM}·${C_RESET}  ${#files[@]} tunnel(s), ${C_GREEN}$running running${C_RESET}"
}

menu() {
    need_root
    need_systemd
    open_input
    trap on_interrupt INT
    while true; do
        banner
        menu_status
        cat <<EOF

   ${C_TEAL}1${C_RESET}) Install or update Kariz
   ${C_TEAL}2${C_RESET}) New tunnel
   ${C_TEAL}3${C_RESET}) List tunnels
   ${C_TEAL}4${C_RESET}) Start / stop / restart / status of a tunnel
   ${C_TEAL}5${C_RESET}) Logs of a tunnel
   ${C_TEAL}6${C_RESET}) Speed test a tunnel (on the entry side)
   ${C_TEAL}7${C_RESET}) Edit a tunnel
   ${C_TEAL}8${C_RESET}) Remove a tunnel
   ${C_TEAL}9${C_RESET}) Uninstall Kariz
   ${C_TEAL}w${C_RESET}) Web panel and agent
   ${C_TEAL}0${C_RESET}) Exit         ${C_DIM}(Ctrl+C: back to the menu, or out from here)${C_RESET}

EOF
        local choice
        ask choice "Choose"
        MENU_BUSY=1 INTERRUPTED=0
        # Each action runs in a subshell, so an error or Ctrl+C returns to the menu.
        case $choice in
            1) (if [[ -x "$BIN" ]]; then cmd_update; else cmd_install; fi) || true ;;
            2) (wizard_add) || true ;;
            3) (cmd_list) || true ;;
            4) (
                pick_tunnel name
                choose action "Action" restart \
                    "start|start it, and at every boot" \
                    "stop|stop it, and not at boot" \
                    "restart|restart it" \
                    "status|its connection, round trip and traffic"
                cmd_service "$action" "$name"
            ) || true ;;
            5) (pick_tunnel name && cmd_service logs "$name") || true ;;
            6) (pick_tunnel name && cmd_speedtest "$name") || true ;;
            7) (pick_tunnel name && cmd_edit "$name") || true ;;
            8) (pick_tunnel name && cmd_remove "$name") || true ;;
            9) (cmd_uninstall) || true ;;
            w | W) (menu_panel) || true ;;
            0 | q) exit 0 ;;
            "") ;;
            *) warn "Choose 0-9 or w." ;;
        esac
        # Wait for Enter before the menu hides what the action printed, unless it was
        # left with Ctrl+C. The wait runs in a subshell too: Ctrl+C there returns at once.
        if ((!INTERRUPTED)) && [[ -n "$choice" && "$choice" != 5 ]]; then
            (
                printf '\n  %sEnter: back to the menu%s' "$C_DIM" "$C_RESET"
                read -r -u "$IN_FD" _
            ) || true
        fi
        MENU_BUSY=0
    done
}

usage() {
    banner
    cat <<EOF
  Usage: kariz-manager [command]      (no command: the menu)

  install [--version vX.Y.Z] [--binary PATH]   install Kariz (latest release by default)
  update  [--version vX.Y.Z]                   update Kariz and restart running tunnels
  add NAME --role entry|exit --mode reverse|direct --transport T [options]
      --listen PORT | --remote ADDR[:PORT]      the listening or the dialing side
                           ADDR: IPv4, IPv6 (brackets optional) or a domain
      --token TOKEN        default: a new one    --profile balanced|ultraspeed|gaming
      --ports LIST         entry only: 443,8080-8090,2053=53,3000-3005=4000-4005
      --protocol tcp|udp|tcp+udp  for --ports    --to HOST   target (127.0.0.1)
      --ipv4-only          listen on IPv4 only (default: IPv4 and IPv6 where there is IPv6)
      --forward LISTEN=TARGET[/tcp|udp|tcp+udp] one rule in full, can repeat
      --ws-path /PATH      ws / wss              --pin HEX   wss dialer
  list                                         all tunnels and their state
  status [NAME] [--watch]                      connection, round trip, traffic per port
                                               (every tunnel without NAME)
  start | stop | restart | logs NAME
  speedtest NAME [--seconds N] [--streams N] [--no-udp]   speed and latency (entry side)
  edit NAME                                    edit, check and restart
  remove NAME [--yes]
  uninstall [--yes]                            also deletes the configs with --yes
  panel install [--port N] [--host H]          the web panel on this server (docs/panel.md)
  panel link | password [--stdin] | status | logs | uninstall [--yes]
  --agent CODE [--version V]                   connect this server to a panel
  agent status | logs | remove
  net list | status NAME                       the private network (GRE) links of this server

  Set GITHUB_TOKEN if GitHub limits your downloads.
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
        start | stop | restart | logs) need_root && cmd_service "$1" "${2:-}" ;;
        status) shift && cmd_status "$@" ;;
        speedtest) shift && cmd_speedtest "$@" ;;
        panel) shift && cmd_panel "$@" ;;
        agent) shift && cmd_agent "$@" ;;
        net) shift && cmd_net "$@" ;;
        --agent) shift && agent_join "$@" ;;
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
