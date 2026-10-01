#!/bin/sh
# Installs pigeon on Linux or macOS: Rust if it is missing, then pigeon,
# built by cargo from the clone of main and into the build folder that
# `pigeon update` reuses, then `pigeon setup`, which asks the rest. With `server
# --key <key> --member <name>`, it asks nothing and sets the machine up as
# an always-on server of the group: started at boot, following everything
# and keeping its history.
#
#   curl -sSf https://raw.githubusercontent.com/mariogeiger/pigeon/main/install.sh | sh
#   curl -sSf https://raw.githubusercontent.com/mariogeiger/pigeon/main/install.sh | sh -s -- server --key <key> --member server

set -eu

fail() {
    echo "✗ $1" >&2
    exit 1
}

# The command that installs a C compiler here, which cargo needs.
compiler_fix() {
    if [ "$(uname -s)" = Darwin ]; then
        echo "xcode-select --install"
    elif command -v apt-get >/dev/null 2>&1; then
        echo "sudo apt-get install build-essential"
    elif command -v dnf >/dev/null 2>&1; then
        echo "sudo dnf install gcc"
    elif command -v pacman >/dev/null 2>&1; then
        echo "sudo pacman -S base-devel"
    else
        echo "installez gcc ou clang"
    fi
}

main() {
    mode=setup
    key=
    member=
    while [ $# -gt 0 ]; do
        case $1 in
            server) mode=server ;;
            --key) key=${2-} && shift ;;
            --member) member=${2-} && shift ;;
            *) fail "argument inconnu : $1" ;;
        esac
        shift
    done
    if [ "$mode" = server ] && { [ -z "$key" ] || [ -z "$member" ]; }; then
        fail "server demande --key et --member"
    fi
    case $(uname -s) in
        Linux) cache=${XDG_CACHE_HOME:-$HOME/.cache} ;;
        Darwin) cache=$HOME/Library/Caches ;;
        *) fail "install.sh installe pigeon sur Linux et macOS" ;;
    esac
    repository=https://github.com/mariogeiger/pigeon
    shell_path=$PATH
    bin=${CARGO_HOME:-$HOME/.cargo}/bin
    PATH=$bin:$PATH

    command -v git >/dev/null 2>&1 || fail "git : installez-le depuis https://git-scm.com"
    command -v cc >/dev/null 2>&1 || fail "Compilateur C : $(compiler_fix)"
    if ! command -v cargo >/dev/null 2>&1; then
        if [ "$mode" = setup ]; then
            printf 'Rust manque : l’installer avec rustup ? [O/n] '
            read -r answer </dev/tty
            case $answer in
                "" | [oOyY]*) ;;
                *) fail "Rust : curl https://sh.rustup.rs -sSf | sh" ;;
            esac
        fi
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
            sh -s -- -y --profile minimal --no-modify-path </dev/null
    fi
    echo "✓ Rust  $(cargo --version)"

    source=$cache/pigeon/source
    if [ -d "$source/.git" ]; then
        git -C "$source" fetch --quiet "$repository" main
        git -C "$source" checkout --quiet --force --detach FETCH_HEAD
    else
        git clone --quiet --branch main "$repository" "$source"
    fi
    cargo install --locked --target-dir "$cache/pigeon/build" --path "$source/crates/pigeon" </dev/null
    echo "✓ pigeon compilé  $("$bin/pigeon" --version | sed 's/^pigeon //')"
    pigeon=$bin/pigeon
    if [ "$(PATH=$shell_path command -v pigeon || true)" = "$pigeon" ]; then
        echo "✓ installé  $pigeon"
    else
        echo "✗ installé  $bin n’est pas dans le PATH : . \"${bin%/bin}/env\""
    fi

    if [ "$mode" = server ]; then
        "$pigeon" service install --linger </dev/null
        "$pigeon" group join --key "$key" --member "$member" </dev/null
        "$pigeon" selection follow --pattern '*' </dev/null
        "$pigeon" retention set --everything on </dev/null
        echo "✓ serveur du groupe, sous le nom $member"
    else
        exec "$pigeon" setup </dev/tty
    fi
}

main "$@"
