#!/usr/bin/env bash
# Kariz manager: installs Kariz and its web panel on a Linux server with systemd, and
# connects the server to a panel. Tunnels, servers, private networks and updates are made
# in the web panel (docs/panel.md), never here.
#
# One line, as root:
#   bash <(curl -fsSL https://raw.githubusercontent.com/Erfan-XRay/Kariz/main/scripts/kariz.sh)
#
# Without arguments it opens a menu. The same actions as commands (see `help`):
#   kariz-manager install [--version vX.Y.Z] [--binary PATH]
#   kariz-manager update | uninstall [--yes]
#   kariz-manager panel install [--domain D | --ip ADDRESS] | link | password | status | logs | uninstall
#   kariz-manager panel cert [--domain D | --ip ADDRESS]   change the domain or address of the panel
#   kariz-manager --agent CODE [--yes]   connect this server to a panel (its join code); a
#                                        server that is connected already is switched to it
#   kariz-manager agent status | logs | remove
#   kariz-manager status              what runs here, and this server's addresses
#
# If GitHub limits your downloads, set GITHUB_TOKEN to a personal access token: it lifts the limit.
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
# What the panel's certificate is for (a domain or an IP address), and what was stopped
# for a moment while Let's Encrypt looked at port 80.
PANEL_DOMAIN=$PANEL_DIR/domain
ACME_STOPPED=/run/kariz-acme-stopped
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

# A line of $1 box-drawing dashes.
rule() {
    local line
    printf -v line '%*s' "${1:-52}" ''
    printf '%s' "${line// /─}"
}

banner() {
    printf '\n%s' "$C_BOLD$C_TEAL"
    printf '  %s\n' ' _  __   _   ___  ___  ____' '| |/ /  /_\ | _ \|_ _||_  /'
    printf '%s' "$C_AQUA"
    printf '  %s\n' "| ' <  / _ \\|   / | |  / / " '|_|\_\/_/ \_\_|_\|___|/___|'
    printf '%s' "$C_RESET"
    printf '  %sKariz manager%s  %s·%s  %sgithub.com/%s%s\n' \
        "$C_BOLD$C_SAND" "$C_RESET" "$C_DIM" "$C_RESET" "$C_AQUA" "$REPO" "$C_RESET"
    printf '  %s%s%s\n' "$C_DIM" "$(rule 54)" "$C_RESET"
}

# A heading for a part of a longer job.
section() {
    printf '\n  %s%s%s %s%s%s\n' "$C_TEAL" "▌" "$C_RESET" "$C_BOLD" "$*" "$C_RESET"
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

# ---- Download and install ----

arch() {
    case "$(uname -m)" in
        x86_64 | amd64) echo x86_64 ;;
        aarch64 | arm64) echo aarch64 ;;
        armv7l | armv7*) echo armv7 ;;
        *) die "No Kariz build for this CPU ($(uname -m))." ;;
    esac
}

# The key releases are signed with. Only the release workflow has the
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

# curl or wget, with the token if there is one. $1: URL, $2: output file or
# - for stdout, $3: extra header (optional).
fetch() {
    local url=$1 out=$2 header=${3:-}
    local auth=()
    [[ -n "${GITHUB_TOKEN:-}" ]] && auth=(-H "Authorization: Bearer $GITHUB_TOKEN")
    if command -v curl >/dev/null; then
        local extra=()
        [[ -n "$header" ]] && extra=(-H "$header")
        # FETCH_MAX: seconds one request may take (set for a quick look, never for a download).
        [[ -n "${FETCH_MAX:-}" ]] && extra+=(--max-time "$FETCH_MAX")
        curl -fsSL --retry 3 "${auth[@]}" "${extra[@]}" -o "$out" "$url"
    elif command -v wget >/dev/null; then
        local extra=()
        [[ -n "${GITHUB_TOKEN:-}" ]] && extra+=(--header="Authorization: Bearer $GITHUB_TOKEN")
        [[ -n "$header" ]] && extra+=(--header="$header")
        [[ -n "${FETCH_MAX:-}" ]] && extra+=(--timeout="$FETCH_MAX")
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
# API, otherwise the public download link.
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
        [[ -n "$version" ]] || die "Could not find the latest release (GitHub may be limiting you: set GITHUB_TOKEN)."
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

# The version number of an installed program ("kariz 1.4.0" gives 1.4.0), or nothing.
bin_version() {
    { "$1" --version 2>/dev/null || true; } | sed -n 's/^[^0-9]*\([0-9][0-9.]*\).*/\1/p' | head -n 1
}

# Looks for a newer release, and says what here is behind it. Returns 1 (and says nothing)
# when nothing is, when GitHub does not answer in a few seconds, or with
# KARIZ_NO_UPDATE_CHECK set. LATEST and BEHIND are left for the caller.
update_available() {
    [[ -z "${KARIZ_NO_UPDATE_CHECK:-}" ]] || return 1
    LATEST=$(FETCH_MAX=6 latest_version 2>/dev/null || true)
    LATEST=${LATEST#v}
    [[ -n "$LATEST" ]] || return 1
    BEHIND=""
    local v
    if [[ -x "$BIN" ]]; then
        v=$(bin_version "$BIN")
        if [[ -n "$v" ]] && version_lt "$v" "$LATEST"; then BEHIND="the tunnel core $v"; fi
    fi
    if [[ -x "$PANEL_BIN" ]]; then
        v=$(bin_version "$PANEL_BIN")
        if [[ -n "$v" ]] && version_lt "$v" "$LATEST"; then
            BEHIND="${BEHIND:+$BEHIND, }the panel and agent program $v"
        fi
    fi
    [[ -n "$BEHIND" ]]
}

# Tells the user a newer release is out and offers to update everything here (the core,
# the panel and the agent). Asked only at a terminal: `kariz-manager update` does it by hand.
offer_update() {
    update_available || return 0
    echo
    warn "Kariz $LATEST is out. Here: $BEHIND."
    info "An older agent or panel can refuse what the newer panel sends (an auto tunnel, mux settings)."
    if confirm "Update everything on this server now? (running tunnels restart for a moment)" y; then
        (cmd_update) || warn "The update did not finish: kariz-manager update shows why."
    fi
}

# Puts the newest kariz-manager script in place (a best-effort; the running copy goes on).
refresh_manager() {
    local tmp
    tmp=$(mktemp)
    if FETCH_MAX=20 fetch "$RAW_URL" "$tmp" 2>/dev/null && bash -n "$tmp" 2>/dev/null; then
        install -m 0755 "$tmp" "$MANAGER"
    fi
    rm -f "$tmp"
}

cmd_update() {
    need_root
    need_kariz
    local before local_install=0 a
    before=$("$BIN" --version)
    for a in "$@"; do
        [[ "$a" == --binary || "$a" == --panel-binary ]] && local_install=1
    done
    cmd_install "$@"
    # A copy of this script that is already installed is not replaced by an install: the
    # newest one is fetched (not for an install from files of your own).
    if ((!local_install)) && [[ -z "${KARIZ_NO_UPDATE_CHECK:-}" && "$(realpath "${BASH_SOURCE[0]}" 2>/dev/null)" == "$MANAGER" ]]; then
        refresh_manager
    fi
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

# Removes the private network links (GRE interfaces named kz-...) the panel made here.
remove_net_links() {
    local link
    command -v ip >/dev/null || return 0
    for link in $({ ip -o link show 2>/dev/null || true; } | sed -n 's/^[0-9]*: \(kz-[a-z0-9]*\)[@:].*/\1/p'); do
        ip link del "$link" 2>/dev/null && info "Removed the private network link $link."
    done
    return 0
}

# Removes the agent completely: its service, settings and identity, what it remembers, the
# update files, and the private network links it made.
agent_remove() {
    systemctl disable --now kariz-agent 2>/dev/null || true
    rm -f "$AGENT_UNIT" "$AGENT_CONF" "$PANEL_DIR/link-transport" "$PANEL_DIR/net.toml" "$PANEL_DIR/connected"
    rm -rf "$PANEL_DIR/updates"
    systemctl daemon-reload
    remove_net_links
}

# Removes Kariz from this server completely: the tunnels, the agent, the web panel and the
# programs. It asks once (--yes: no questions, and the configs and the panel's data go too).
cmd_uninstall() {
    need_root
    local yes=${1:-} purge=0 unit tunnels=0
    tunnels=$(systemctl list-unit-files --plain --no-legend 'kariz@*' 2>/dev/null | grep -vc '@\.service' || true)
    if [[ "$yes" != --yes ]]; then
        echo
        warn "This removes Kariz from this server completely:"
        info "every tunnel on it ($tunnels) is stopped and removed,"
        has_agent && info "its agent is removed: the server disconnects from its panel (remove it in the panel too),"
        [[ -f "$PANEL_UNIT" || -f "$PANEL_CONF" ]] && info "the web panel on it is removed,"
        info "and the kariz, kariz-panel and kariz-manager programs go."
        confirm "Remove Kariz completely?" || return 0
        confirm "Also delete the tunnel configs and the panel's data (tokens, servers, certificate)?" && purge=1
    else
        purge=1
    fi
    for unit in $(systemctl list-unit-files --plain --no-legend 'kariz@*' | awk '{print $1}') \
        $(systemctl list-units --all --plain --no-legend 'kariz@*' | awk '{print $1}'); do
        [[ "$unit" == *@.service ]] && continue
        systemctl disable --now "$unit" 2>/dev/null || true
    done
    rm -f "$UNIT" "$BIN"
    if has_agent || [[ -f "$AGENT_UNIT" ]]; then
        agent_remove
        ok "The agent is removed."
    fi
    if [[ -f "$PANEL_UNIT" || -f "$PANEL_CONF" ]]; then
        if ((purge)); then panel_uninstall --yes; else panel_uninstall --yes --keep-data; fi
    fi
    rm -f "$PANEL_BIN"
    systemctl daemon-reload
    if ((purge)); then
        rm -rf "$CONF_DIR" "$PANEL_DIR" "$PANEL_DATA"
    fi
    ok "Kariz is removed."
    rm -f "$MANAGER"
}

# ---- Addresses ----

# The server's own addresses, as defaults for the address the panel is reached at.
own_ip4() {
    { ip -4 route get 1.1.1.1 2>/dev/null || true; } | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -n 1
}

own_ip6() {
    { ip -6 route get 2606:4700:4700::1111 2>/dev/null || true; } | sed -n 's/.* src \([0-9a-fA-F:]*\).*/\1/p' | head -n 1
}

# Whether $1 is a global IPv6 address (not link-local, unique local or loopback).
public_ip6() {
    local a=${1,,}
    [[ "$a" == *:* ]] || return 1
    case $a in
        ::1 | :: | fe[89ab]* | f[cd]* | ff*) return 1 ;;
    esac
    return 0
}

# This server on one line: its name and the IPv4 and IPv6 addresses it has.
server_line() {
    local v4 v6
    v4=$(own_ip4)
    v6=$(own_ip6)
    printf '  %s%s%s' "$C_BOLD" "$(hostname 2>/dev/null || echo this-server)" "$C_RESET"
    if [[ -n "$v4" ]]; then
        printf '   %sIPv4%s %s' "$C_DIM" "$C_RESET" "$v4"
        public_ip4 "$v4" || printf ' %s(private)%s' "$C_DIM" "$C_RESET"
    fi
    if [[ -n "$v6" ]]; then
        printf '   %sIPv6%s %s' "$C_DIM" "$C_RESET" "$v6"
        public_ip6 "$v6" || printf ' %s(private)%s' "$C_DIM" "$C_RESET"
    fi
    [[ -n "$v4$v6" ]] || printf '   %sno network address found%s' "$C_DIM" "$C_RESET"
    echo
}

# ---- The web panel and its agent ----

# The panel's address, certificate and ports, from its settings.
panel_setting() { sed -n "s/^$1 = \"\(.*\)\"/\1/p" "$PANEL_CONF" | head -n 1; }

# This server's address as another machine would use it: --host, else its IPv4, else IPv6.
panel_host() {
    local host=${1:-}
    [[ -n "$host" || ! -s "$PANEL_DOMAIN" ]] || host=$(head -n 1 "$PANEL_DOMAIN")
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
    if [[ -n "$(panel_setting cert_file)" ]]; then
        printf '  %s certificate %s %s\n' "$C_TEAL" "$C_RESET" "$(panel_setting cert_file)"
        printf '  %s             %s (not self-signed: no browser warning)\n' "$C_DIM" "$C_RESET"
    elif [[ -n "$fingerprint" ]]; then
        printf '  %s certificate %s SHA-256 %s\n' "$C_TEAL" "$C_RESET" "$fingerprint"
        printf '  %s             %s (self-signed: the browser warns; compare this fingerprint)\n' "$C_DIM" "$C_RESET"
        printf '  %s             %s (no warning with a real one: kariz-manager panel cert)\n' "$C_DIM" "$C_RESET"
    fi
    if [[ -n "$agent" ]]; then
        printf '  %s agents      %s port %s: TCP and UDP, and TCP %s (open them for the servers you add)\n' "$C_TEAL" "$C_RESET" "${agent##*:}" "$((${agent##*:} + 1))"
    fi
    printf '  %s sign in     %s ' "$C_TEAL" "$C_RESET"
    "$PANEL_BIN" login-link -c "$PANEL_CONF" --host "$host" 2>/dev/null
    printf '  %s             %s (works once, for 60 minutes; make another with: kariz-manager panel link)\n' "$C_DIM" "$C_RESET"
}

# Whether $1 can be a server's name in the panel.
valid_server_name() { [[ "$1" =~ ^[A-Za-z0-9._-]{1,40}$ ]]; }

# Asks what this server is called in the panel and puts the answer in the variable named $1.
# The host name is the default; Enter keeps it.
ask_server_name() {
    local _var=$1 _def _reply
    _def=$(hostname 2>/dev/null | tr -c 'A-Za-z0-9._\n-' '-' | cut -c1-40)
    [[ -n "$_def" ]] || _def=panel
    while true; do
        ask _reply "What should this server be called in the panel?" "$_def"
        if valid_server_name "$_reply"; then
            printf -v "$_var" '%s' "$_reply"
            return
        fi
        warn "Letters, digits, - _ . and at most 40 characters."
    done
}

panel_install() {
    need_root
    need_systemd
    local port="" host="" domain="" ip="" email="" yes=0 cert="" key="" name="" version=()
    while [[ $# -gt 0 ]]; do
        case $1 in
            --port) port=$2 && shift 2 ;;
            --name) name=${2:-} && shift 2 ;;
            --host) host=$2 && shift 2 ;;
            --domain) domain=${2:-} && shift 2 ;;
            --ip) ip=${2:-} && shift 2 ;;
            --email) email=${2:-} && shift 2 ;;
            --cert-file) cert=${2:-} && shift 2 ;;
            --key-file) key=${2:-} && shift 2 ;;
            --yes) yes=1 && shift ;;
            --version) version=(--version "$2") && shift 2 ;;
            *) die "panel install: unknown option $1" ;;
        esac
    done
    [[ -z "$cert" && -z "$key" ]] || [[ -f "$cert" && -f "$key" ]] ||
        die "--cert-file and --key-file go together, and both must be files."
    if [[ ! -x "$PANEL_BIN" ]]; then
        cmd_install "${version[@]}"
    fi
    [[ -x "$PANEL_BIN" ]] ||
        die "This Kariz release has no web panel. Install 0.8 or later: kariz-manager update"
    # What this server is called in the panel: asked (the host name is the default), or given
    # with --name; with --yes the host name stays unless --name says otherwise.
    if [[ -z "$name" ]] && ((!yes)) && [[ ! -f "$PANEL_CONF" ]]; then
        ask_server_name name
    fi
    [[ -z "$name" ]] || valid_server_name "$name" ||
        die "The name is letters, digits, - _ . and at most 40 characters."
    local init_args=(-c "$PANEL_CONF" --data-dir "$PANEL_DATA")
    [[ -z "$name" ]] || init_args+=(--name "$name")
    if [[ -n "$port" ]]; then
        init_args+=(--port "$port")
    fi
    mkdir -p "$PANEL_DIR" "$PANEL_DATA"
    chmod 700 "$PANEL_DIR" "$PANEL_DATA"
    # A new panel gets its certificate first (the panel is only ever served over TLS that
    # the browser trusts); one that is already set up keeps what it has.
    if [[ ! -f "$PANEL_CONF" ]]; then
        if [[ -n "$cert" ]]; then
            init_args+=(--cert-file "$(own_file "$cert")" --key-file "$(own_file "$key")")
        else
            choose_identity "$domain" "$ip" "$yes" || die "Cancelled."
            if [[ -z "$email" ]] && ((!yes)); then
                ask email "Email for expiry notices (optional, Enter to skip)" ""
            fi
            get_cert "$CERT_IDENTITY" "$CERT_KIND" "$email" "$yes"
            init_args+=(--cert-file "$CERT_LIVE/fullchain.pem" --key-file "$CERT_LIVE/privkey.pem")
            printf '%s
' "$CERT_IDENTITY" >"$PANEL_DOMAIN"
        fi
    fi
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
    echo
    info "Next: open the sign-in link above. Servers, Add server shows the command that connects another server."
}

# ---- The panel's certificate (Let's Encrypt) ----
#
# The panel is always served over a certificate the browser trusts: for a domain name, or
# for the server's own public IP address (Let's Encrypt issues those for 6 days at a time).
# Both are renewed by a timer, and the panel loads the new one without a restart.

CERTBOT_VENV=/opt/kariz-certbot
RENEW_SERVICE=/etc/systemd/system/kariz-cert-renew.service
RENEW_TIMER=/etc/systemd/system/kariz-cert-renew.timer
# IP address certificates need certbot 5.4 or later, newer than most distributions carry.
CERTBOT_MIN=5.4.0

# Who listens on TCP port $1, one process per line: "unit comm pid" (unit is - when the
# process is not a systemd service).
port_users() {
    local pids pid unit comm
    pids=$({ ss -H -ltnp "sport = :$1" 2>/dev/null || true; } | { grep -o 'pid=[0-9]*' || true; } | sort -u)
    for pid in $pids; do
        pid=${pid#pid=}
        unit=$({ sed -n 's#.*/\([^/]*\.service\)$#\1#p' "/proc/$pid/cgroup" 2>/dev/null || true; } | head -n 1)
        comm=$(cat "/proc/$pid/comm" 2>/dev/null || echo "?")
        printf '%s %s %s\n' "${unit:--}" "$comm" "$pid"
    done
}

# What certbot runs around a request: `pre` stops the services that hold port 80 (Let's
# Encrypt must reach it), `post` starts them again, `deploy` makes the panel load the new
# certificate. The same three run at every renewal, so it keeps working by itself.
cert_hook() {
    local unit comm pid stop=""
    case ${1:-} in
        pre)
            while read -r unit comm pid; do
                [[ -n "${pid:-}" ]] || continue
                if [[ "$unit" == - || "$comm" == docker-proxy ]]; then
                    warn "Port 80 is used by $comm (pid $pid), which this script cannot stop and start again."
                    exit 1
                fi
                [[ " $stop " == *" $unit "* ]] || stop="$stop $unit"
            done < <(port_users 80)
            : >"$ACME_STOPPED"
            for unit in $stop; do
                info "Stopping $unit for a moment (port 80)"
                systemctl stop "$unit"
                echo "$unit" >>"$ACME_STOPPED"
            done
            ;;
        post)
            if [[ -f "$ACME_STOPPED" ]]; then
                while read -r unit; do
                    [[ -n "$unit" ]] || continue
                    info "Starting $unit again"
                    systemctl start "$unit" || warn "Could not start $unit: start it yourself."
                done <"$ACME_STOPPED"
                rm -f "$ACME_STOPPED"
            fi
            ;;
        deploy) systemctl kill -s HUP kariz-panel 2>/dev/null || true ;;
        *) die "cert-hook: pre | post | deploy" ;;
    esac
}

# The absolute path of a certificate or key file of your own. The panel service has a
# private /tmp, so a file there would be invisible to it.
own_file() {
    local f
    f=$(realpath "$1") || die "No such file: $1"
    case $f in
        /tmp/* | /var/tmp/*) die "$f is in /tmp, which the panel service does not see: put it somewhere else (like /etc/kariz-panel/)." ;;
    esac
    printf '%s' "$f"
}

# Sets (or, with no arguments, clears) the panel's own certificate in its settings.
panel_set_cert() {
    local tmp
    tmp=$(mktemp)
    { grep -v -e '^cert_file *=' -e '^key_file *=' "$PANEL_CONF" || true; } >"$tmp"
    if [[ $# -eq 2 ]]; then
        printf 'cert_file = "%s"\nkey_file = "%s"\n' "$1" "$2" >>"$tmp"
    fi
    install -m 0600 "$tmp" "$PANEL_CONF"
    rm -f "$tmp"
}

# Whether the certbot at $1 is new enough.
certbot_ok() {
    local v
    v=$({ "$1" --version 2>&1 || true; } | sed -n 's/^certbot \([0-9][0-9.]*\).*/\1/p')
    [[ -n "$v" ]] && ! version_lt "$v" "$CERTBOT_MIN"
}

# The certbot to use (its path in CERTBOT): the system's if it is new enough, else one of
# our own in a virtualenv, installed with pip.
ensure_certbot() {
    local c
    for c in "$CERTBOT_VENV/bin/certbot" "$(command -v certbot || true)"; do
        if [[ -n "$c" && -x "$c" ]] && certbot_ok "$c"; then
            CERTBOT=$c
            return 0
        fi
    done
    info "This needs certbot $CERTBOT_MIN or later; installing it in $CERTBOT_VENV (python, pip)."
    # Try first: many systems have python with venv already. Where the venv module is a
    # separate package (Debian, Ubuntu), the failure is what tells us so.
    if ! { command -v python3 >/dev/null && rm -rf "$CERTBOT_VENV" && python3 -m venv "$CERTBOT_VENV"; }; then
        info "Installing python with venv and pip."
        if command -v apt-get >/dev/null; then
            apt-get install -y --no-install-recommends python3 python3-venv
        elif command -v dnf >/dev/null; then
            dnf install -y --setopt=install_weak_deps=False python3 python3-pip
        elif command -v yum >/dev/null; then
            yum install -y python3 python3-pip
        elif command -v apk >/dev/null; then
            apk add python3 py3-pip
        else
            die "Install python3 (with venv) yourself, then run this again."
        fi
        rm -rf "$CERTBOT_VENV"
        python3 -m venv "$CERTBOT_VENV" || die "Could not make a python virtualenv in $CERTBOT_VENV (see above)."
    fi
    "$CERTBOT_VENV/bin/pip" install --quiet --upgrade pip certbot ||
        die "pip could not install certbot."
    certbot_ok "$CERTBOT_VENV/bin/certbot" ||
        die "The certbot that got installed is older than $CERTBOT_MIN: update python or install certbot yourself."
    CERTBOT=$CERTBOT_VENV/bin/certbot
}

# Whether $1 is a public IPv4 address: the kind Let's Encrypt can certify.
public_ip4() {
    local a b
    [[ "$1" =~ ^([0-9]{1,3})\.([0-9]{1,3})\.[0-9]{1,3}\.[0-9]{1,3}$ ]] || return 1
    a=${BASH_REMATCH[1]} b=${BASH_REMATCH[2]}
    ((a < 256 && b < 256)) || return 1
    ((a == 10 || a == 127 || a == 0 || a >= 224)) && return 1
    ((a == 172 && b >= 16 && b <= 31)) && return 1
    ((a == 192 && b == 168)) && return 1
    ((a == 169 && b == 254)) && return 1
    ((a == 100 && b >= 64 && b <= 127)) && return 1
    return 0
}

# certbot's name for the certificate of one identity (a domain or an IP address).
cert_name() { printf 'kariz-panel-%s' "$(printf '%s' "$1" | tr -c 'a-zA-Z0-9\n' '-')"; }

# The timer that renews the certificate: twice a day, and certbot renews only when it is
# due (an IP address certificate lasts 6 days, so about every 2 to 3 days).
install_renew_timer() {
    cat >"$RENEW_SERVICE" <<EOF
[Unit]
Description=Renew the Kariz panel certificate
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=$CERTBOT renew --non-interactive --quiet
EOF
    cat >"$RENEW_TIMER" <<'EOF'
[Unit]
Description=Renew the Kariz panel certificate

[Timer]
OnCalendar=*-*-* 00,12:00:00
RandomizedDelaySec=15min
Persistent=true

[Install]
WantedBy=timers.target
EOF
    systemctl daemon-reload
    systemctl enable --now kariz-cert-renew.timer >/dev/null
}

# Stops and starts nothing; only says what holds port 80 and asks whether it may be stopped
# for a moment. Returns 1 when it may not.
port80_agreed() {
    local yes=$1 users unit comm pid
    users=$(port_users 80)
    [[ -n "$users" ]] || return 0
    echo
    warn "Port 80 is in use right now:"
    while read -r unit comm pid; do
        printf '      %s (%s, pid %s)\n' "$comm" "${unit/#-/not a service}" "$pid" >&2
    done <<<"$users"
    info "Let's Encrypt has to reach port 80. I can stop it for the few seconds this takes,"
    info "and start it again right after. The same happens each time the certificate is renewed."
    ((yes)) || confirm "Stop it for a moment and start it again afterwards?" y
}

# Gets the certificate for `$1` (a domain, or an IP address with `$2` = ip); the directory
# of its files is left in CERT_LIVE. Not run in $(...): it asks questions.
get_cert() {
    local identity=$1 kind=$2 email=$3 yes=$4
    local name mail=() target=()
    name=$(cert_name "$identity")
    port80_agreed "$yes" || die "Free port 80 (or let me stop it) and run this again."
    ensure_certbot
    [[ -x "$MANAGER" ]] || install_manager
    [[ -x "$MANAGER" ]] || die "The certificate hooks need $MANAGER."
    mail=(--register-unsafely-without-email)
    [[ -z "$email" ]] || mail=(--email "$email")
    if [[ "$kind" == ip ]]; then
        target=(--ip-address "$identity" --preferred-profile shortlived)
    else
        target=(-d "$identity")
    fi
    info "Asking Let's Encrypt for $identity"
    "$CERTBOT" certonly --standalone --preferred-challenges http "${target[@]}" \
        --cert-name "$name" --non-interactive --agree-tos "${mail[@]}" --reuse-key \
        --pre-hook "$MANAGER panel cert-hook pre" \
        --post-hook "$MANAGER panel cert-hook post" \
        --deploy-hook "$MANAGER panel cert-hook deploy" ||
        die "Let's Encrypt did not give a certificate (see above; DNS for a domain, and port 80 reachable from the internet, are the usual reasons)."
    install_renew_timer
    CERT_LIVE=/etc/letsencrypt/live/$name
}

# Asks which it is, a domain or this server's IP address; sets CERT_KIND and CERT_IDENTITY.
choose_identity() {
    local domain=$1 ip=$2 yes=$3
    CERT_KIND="" CERT_IDENTITY=""
    if [[ -n "$domain" ]]; then
        CERT_KIND=domain CERT_IDENTITY=$domain
    elif [[ -n "$ip" ]]; then
        CERT_KIND=ip CERT_IDENTITY=$ip
    else
        local pick
        choose pick "How will you open the panel?" domain \
            "domain|I have a domain name that points at this server" \
            "ip|only this server's IP address (a 6-day certificate, renewed by itself)"
        CERT_KIND=$pick
    fi
    if [[ "$CERT_KIND" == domain ]]; then
        [[ -n "$CERT_IDENTITY" ]] || ask CERT_IDENTITY "Domain name (like panel.example.com)"
        [[ "$CERT_IDENTITY" =~ ^([a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?\.)+[a-zA-Z]{2,}$ ]] ||
            die "That is not a domain name: $CERT_IDENTITY"
        local resolved mine
        resolved=$({ getent ahostsv4 "$CERT_IDENTITY" 2>/dev/null || true; } | awk 'NR==1{print $1}')
        mine=$(own_ip4)
        if [[ -z "$resolved" ]]; then
            warn "$CERT_IDENTITY does not resolve yet: add its DNS record (an A record to ${mine:-this server}) first."
            ((yes)) || confirm "Try anyway?" || return 1
        elif [[ -n "$mine" && "$resolved" != "$mine" ]]; then
            warn "$CERT_IDENTITY points at $resolved, but this server is $mine."
            ((yes)) || confirm "Try anyway?" || return 1
        fi
    else
        [[ -n "$CERT_IDENTITY" ]] || CERT_IDENTITY=$(own_ip4)
        if [[ -z "${ip:-}" ]] && ((!yes)); then
            ask CERT_IDENTITY "This server's public IP address" "$CERT_IDENTITY"
        fi
        public_ip4 "$CERT_IDENTITY" ||
            die "$CERT_IDENTITY is not a public IPv4 address: Let's Encrypt cannot certify it. Use a domain, or the address the internet reaches this server at (--ip ADDRESS)."
    fi
}

# `panel cert`: gets a certificate and makes the panel use it. Run again to change the
# domain or the address.
panel_cert() {
    need_root
    need_systemd
    [[ -f "$PANEL_CONF" ]] || die "The panel is not installed: kariz-manager panel install"
    local domain="" ip="" email="" yes=0 cert="" key=""
    while [[ $# -gt 0 ]]; do
        case $1 in
            --domain) domain=${2:-} && shift 2 ;;
            --ip) ip=${2:-} && shift 2 ;;
            --email) email=${2:-} && shift 2 ;;
            --cert-file) cert=${2:-} && shift 2 ;;
            --key-file) key=${2:-} && shift 2 ;;
            --yes) yes=1 && shift ;;
            *) die "panel cert: [--domain D | --ip ADDRESS] [--email E] [--yes] | --cert-file F --key-file K" ;;
        esac
    done
    if [[ -n "$cert$key" ]]; then
        # A certificate of your own (not renewed by Kariz).
        [[ -f "$cert" && -f "$key" ]] || die "--cert-file and --key-file go together, and both must be files."
        panel_set_cert "$(own_file "$cert")" "$(own_file "$key")"
        rm -f "$PANEL_DOMAIN"
        systemctl restart kariz-panel
        ok "The panel uses your certificate now. Kariz does not renew it: send the panel a SIGHUP after you do (systemctl kill -s HUP kariz-panel)."
        return 0
    fi
    choose_identity "$domain" "$ip" "$yes" || return 0
    if [[ -z "$email" ]] && ((!yes)); then
        ask email "Email for expiry notices (optional, Enter to skip)" ""
    fi
    local old=""
    get_cert "$CERT_IDENTITY" "$CERT_KIND" "$email" "$yes"
    if [[ -s "$PANEL_DOMAIN" ]]; then old=$(cert_name "$(head -n 1 "$PANEL_DOMAIN")"); fi
    panel_set_cert "$CERT_LIVE/fullchain.pem" "$CERT_LIVE/privkey.pem"
    printf '%s\n' "$CERT_IDENTITY" >"$PANEL_DOMAIN"
    systemctl restart kariz-panel
    sleep 2
    systemctl is-active --quiet kariz-panel ||
        die "The panel did not start with the new certificate: journalctl -u kariz-panel -n 50"
    # The certificate of what it was before is not needed any more (and must not be renewed).
    if [[ -n "$old" && "$old" != "$(cert_name "$CERT_IDENTITY")" ]]; then
        "$CERTBOT" delete --cert-name "$old" --non-interactive >/dev/null 2>&1 || true
    fi
    ok "The panel has a trusted certificate for $CERT_IDENTITY; it renews by itself."
    panel_show "$CERT_IDENTITY"
}

# `tunnel-cert`: a Let's Encrypt certificate for a wss tunnel, with the same machinery as the
# panel's (port 80 is freed for a moment, the renewal timer does the rest). The web panel runs
# it on the server a tunnel listens on; it prints the files as `cert=` and `key=` lines.
cmd_tunnel_cert() {
    need_root
    need_systemd
    local domain="" ip="" email=""
    while [[ $# -gt 0 ]]; do
        case $1 in
            --domain) domain=${2:-} && shift 2 ;;
            --ip) ip=${2:-} && shift 2 ;;
            --email) email=${2:-} && shift 2 ;;
            *) die "tunnel-cert: --domain D | --ip ADDRESS [--email E]" ;;
        esac
    done
    [[ -n "$domain$ip" ]] || die "tunnel-cert: --domain D | --ip ADDRESS [--email E]"
    choose_identity "$domain" "$ip" 1 || die "Cancelled."
    get_cert "$CERT_IDENTITY" "$CERT_KIND" "$email" 1
    printf 'cert=%s/fullchain.pem
key=%s/privkey.pem
' "$CERT_LIVE" "$CERT_LIVE"
}

cmd_panel() {
    local action=${1:-}
    shift || true
    case $action in
        install) panel_install "$@" ;;
        name)
            need_root
            [[ -f "$PANEL_CONF" ]] || die "The panel is not installed: kariz-manager panel install"
            local new=${1:-}
            [[ -n "$new" ]] || ask_server_name new
            valid_server_name "$new" || die "The name is letters, digits, - _ . and at most 40 characters."
            "$PANEL_BIN" init -c "$PANEL_CONF" --data-dir "$PANEL_DATA" --name "$new" >/dev/null
            ok "This server is called $new in the panel."
            ;;
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
        cert) panel_cert "$@" ;;
        cert-hook) cert_hook "$@" ;;
        uninstall) panel_uninstall "$@" ;;
        *) die "panel: install [--port N] [--domain D | --ip A] [--name NAME] | name [NAME] | link | password [--stdin] | cert [--domain D | --ip A] | status | logs | uninstall [--yes]" ;;
    esac
}

panel_uninstall() {
    need_root
    local yes="" keep=0 a
    for a in "$@"; do
        case $a in
            --yes) yes=--yes ;;
            --keep-data) keep=1 ;;
        esac
    done
    if [[ "$yes" != --yes ]]; then
        confirm "Stop the web panel and remove it?" || return 0
    fi
    systemctl disable --now kariz-panel 2>/dev/null || true
    if [[ -s "$PANEL_DOMAIN" ]]; then
        # Its Let's Encrypt certificate is not renewed any more.
        systemctl disable --now kariz-cert-renew.timer 2>/dev/null || true
        rm -f "$RENEW_SERVICE" "$RENEW_TIMER"
        local c
        for c in "$CERTBOT_VENV/bin/certbot" "$(command -v certbot || true)"; do
            [[ -n "$c" && -x "$c" ]] || continue
            "$c" delete --cert-name "$(cert_name "$(head -n 1 "$PANEL_DOMAIN")")" --non-interactive >/dev/null 2>&1 || true
        done
        rm -f "$PANEL_DOMAIN"
    fi
    rm -f "$PANEL_UNIT"
    systemctl daemon-reload
    if [[ ! -f "$AGENT_CONF" ]]; then
        rm -f "$PANEL_BIN"
    fi
    if ((!keep)) && { [[ "$yes" == --yes ]] || confirm "Also delete the panel's database, certificate and settings (the servers it knows)?"; }; then
        rm -rf "$PANEL_DATA" "$PANEL_CONF"
    fi
    ok "The web panel is removed."
}

# Whether this server already has an agent: its settings, or its service running.
has_agent() {
    [[ -f "$AGENT_CONF" ]] || systemctl is-active --quiet kariz-agent 2>/dev/null
}

# Connects this server to a panel: `kariz-manager --agent CODE`. A server that has an agent
# already is switched to the new panel: the old agent is removed and the new one takes its
# place (asked first, unless --yes).
agent_join() {
    need_root
    need_systemd
    local code=${1:-} version=() yes=0
    shift || true
    while [[ $# -gt 0 ]]; do
        case $1 in
            --version) version=(--version "$2") && shift 2 ;;
            --yes | -y) yes=1 && shift ;;
            *) die "agent: unknown option $1" ;;
        esac
    done
    [[ "$code" == kz1_* ]] || die "That is not a join code (it starts with kz1_). Copy it whole from the panel: Servers, Add server."
    if has_agent; then
        echo
        warn "This server already has a Kariz agent (it is connected to a panel)."
        info "The new code replaces it: the old agent is removed, and the new one takes its place."
        info "The old panel keeps listing this server as offline until you remove it there."
        if ((!yes)); then
            confirm "Replace the old agent with the new one?" y ||
                die "Nothing changed. Run it again with --yes to replace the old agent without asking."
        fi
        systemctl disable --now kariz-agent 2>/dev/null || true
        rm -f "$AGENT_CONF" "$PANEL_DIR/link-transport"
        ok "The old agent is removed."
    fi
    if [[ ! -x "$PANEL_BIN" ]]; then
        cmd_install "${version[@]}"
    elif ((${#version[@]} == 0)) && update_available; then
        # An agent program from an older release may not understand what the panel sends.
        warn "Kariz $LATEST is out, and this server has an older one ($BEHIND): updating it first."
        cmd_install
    fi
    [[ -x "$PANEL_BIN" ]] ||
        die "This Kariz release has no agent. Install 0.8 or later: kariz-manager update, then --agent CODE"
    mkdir -p "$PANEL_DIR"
    chmod 700 "$PANEL_DIR"
    "$PANEL_BIN" agent --join "$code" --no-run -c "$AGENT_CONF"
    install_panel_units
    systemctl enable kariz-agent >/dev/null 2>&1
    # A restart, not just a start: an agent that was running must not keep its old settings.
    systemctl restart kariz-agent
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
            agent_remove
            ok "The agent is removed. Remove the server in the panel too (Servers, Remove). Its tunnels keep running."
            ;;
        *) die "agent: CODE | join CODE | status | logs | remove" ;;
    esac
}

# ---- Status ----

# What runs on this server, in a few lines.
cmd_status() {
    banner
    server_line
    echo
    if [[ ! -x "$BIN" ]]; then
        warn "Kariz is not installed yet: run kariz-manager install."
        return 0
    fi
    local running=0
    running=$(systemctl list-units --type=service --state=active --plain --no-legend 'kariz@*' 2>/dev/null | wc -l)
    printf '  %s core       %s %s  %s(%s tunnel service(s) running here)%s\n' "$C_TEAL" "$C_RESET" "$("$BIN" --version)" "$C_DIM" "$running" "$C_RESET"
    if systemctl is-active --quiet kariz-panel 2>/dev/null; then
        printf '  %s web panel  %s %srunning%s\n' "$C_TEAL" "$C_RESET" "$C_GREEN" "$C_RESET"
    elif [[ -f "$PANEL_CONF" ]]; then
        printf '  %s web panel  %s %sinstalled, not running%s (journalctl -u kariz-panel)\n' "$C_TEAL" "$C_RESET" "$C_YELLOW" "$C_RESET"
    else
        printf '  %s web panel  %s %snot installed%s (kariz-manager panel install)\n' "$C_TEAL" "$C_RESET" "$C_DIM" "$C_RESET"
    fi
    if systemctl is-active --quiet kariz-agent 2>/dev/null; then
        printf '  %s agent      %s %sconnected to a panel%s\n' "$C_TEAL" "$C_RESET" "$C_GREEN" "$C_RESET"
    elif [[ -f "$AGENT_CONF" ]]; then
        printf '  %s agent      %s %sset up, not running%s (journalctl -u kariz-agent)\n' "$C_TEAL" "$C_RESET" "$C_YELLOW" "$C_RESET"
    else
        printf '  %s agent      %s %snot set up%s (kariz-manager --agent CODE)\n' "$C_TEAL" "$C_RESET" "$C_DIM" "$C_RESET"
    fi
    if [[ -f "$PANEL_CONF" ]]; then
        panel_show "$(panel_host "")"
    fi
    if update_available; then
        echo
        warn "Kariz $LATEST is out. Here: $BEHIND. Update: kariz-manager update"
    fi
}

# ---- Menu ----

# The first time on a server: Kariz itself is installed at once, and what else this server
# is for is asked: nothing (tunnels are made from a panel elsewhere), the web panel here, or
# a connection to a panel that exists.
first_run() {
    banner
    server_line
    section "Welcome"
    info "Kariz is not installed on this server yet: installing the core now."
    cmd_install
    section "What else should this server do?"
    local what code
    choose what "Pick one (you can add the others later from this menu)" core \
        "core|only the Kariz core: tunnels are made from a web panel on another server" \
        "panel|also install the web panel here (servers, tunnels and charts in a browser)" \
        "agent|connect this server to a web panel that exists already (needs its join code)"
    case $what in
        panel) panel_install ;;
        agent)
            ask code "Join code (starts with kz1_)"
            agent_join "$code"
            ;;
        *) ok "Done: the Kariz core is installed. Open the menu again with: kariz-manager" ;;
    esac
}

menu_panel() {
    local action code
    choose action "Web panel" install \
        "install|install the web panel on this server" \
        "link|make a one-time login link" \
        "password|set a new admin password" \
        "cert|change the domain or IP address (Let's Encrypt certificate)" \
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

# "kariz 1.4.0 · the web panel is running" for the top of the menu.
menu_status() {
    server_line
    echo
    if [[ ! -x "$BIN" ]]; then
        warn "Kariz is not installed yet: choose 1."
        return
    fi
    local panel="no web panel here"
    if systemctl is-active --quiet kariz-panel; then
        panel="${C_GREEN}the web panel is running${C_RESET}"
    elif systemctl is-active --quiet kariz-agent; then
        panel="${C_GREEN}connected to a panel (agent)${C_RESET}"
    fi
    info "$("$BIN" --version)  ${C_DIM}·${C_RESET}  $panel"
}

menu() {
    need_root
    need_systemd
    open_input
    trap on_interrupt INT
    if [[ ! -x "$BIN" ]]; then
        # An action that fails ends it, as in the menu; the menu opens after it.
        (first_run) || true
    else
        offer_update
    fi
    while true; do
        banner
        menu_status
        cat <<EOF

   ${C_TEAL}1${C_RESET}) Install or update Kariz
   ${C_TEAL}2${C_RESET}) Web panel and agent   ${C_DIM}(tunnels are made there)${C_RESET}
   ${C_TEAL}3${C_RESET}) Uninstall Kariz
   ${C_TEAL}0${C_RESET}) Exit         ${C_DIM}(Ctrl+C: back to the menu, or out from here)${C_RESET}

EOF
        local choice
        ask choice "Choose"
        MENU_BUSY=1 INTERRUPTED=0
        # Each action runs in a subshell, so an error or Ctrl+C returns to the menu.
        case $choice in
            1) (if [[ -x "$BIN" ]]; then cmd_update; else cmd_install; fi) || true ;;
            2) (menu_panel) || true ;;
            3) (cmd_uninstall) || true ;;
            0 | q) exit 0 ;;
            "") ;;
            *) warn "Choose 0-3." ;;
        esac
        # Wait for Enter before the menu hides what the action printed, unless it was
        # left with Ctrl+C or was no action at all (a wrong number shows the menu again).
        # The wait runs in a subshell too: Ctrl+C there returns at once.
        if ((!INTERRUPTED)) && [[ "$choice" =~ ^[1-3]$ ]]; then
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

  Tunnels, servers, private networks and updates of other servers are made in the web
  panel (docs/panel.md). This script installs Kariz and the panel on a server.

  install [--version vX.Y.Z] [--binary PATH]   install Kariz (latest release by default)
  update  [--version vX.Y.Z]                   update Kariz and restart what runs
  uninstall [--yes]                            also deletes the configs with --yes
  panel install [--port N] [--domain D | --ip ADDRESS] [--email E] [--name NAME] [--yes]
                                               the web panel on this server, with a Let's
                                               Encrypt certificate for the domain or IP address
  panel cert [--domain D | --ip ADDRESS]       change its domain or address (renewal is automatic)
  panel cert --cert-file F --key-file K        use a certificate of your own instead
  panel link | password [--stdin] | status | logs | uninstall [--yes]
  --agent CODE [--version V] [--yes]           connect this server to a panel (an agent that is
                                               here already is replaced by the new one)
  agent status | logs | remove
  status                                       what runs here, and this server's addresses
  tunnel-cert --domain D | --ip A [--email E]  a Let's Encrypt certificate for a wss tunnel (the
                                               panel runs this on the server the tunnel listens on)

  Set GITHUB_TOKEN if GitHub limits your downloads.
EOF
}

main() {
    case ${1:-} in
        "") menu ;;
        install) shift && cmd_install "$@" ;;
        update) shift && cmd_update "$@" ;;
        uninstall) cmd_uninstall "${2:-}" ;;
        panel) shift && cmd_panel "$@" ;;
        agent) shift && cmd_agent "$@" ;;
        --agent) shift && agent_join "$@" ;;
        status) cmd_status ;;
        tunnel-cert) shift && cmd_tunnel_cert "$@" ;;
        help | -h | --help) usage ;;
        *) usage && exit 1 ;;
    esac
}

# Not when sourced (the tests load the functions). Piped into bash, there is no source
# file at all.
if [[ "${BASH_SOURCE[0]:-$0}" == "$0" ]]; then
    main "$@"
fi
