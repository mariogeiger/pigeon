#!/bin/sh
# Installs pigeon on Linux or macOS: Rust if it is missing, then pigeon,
# built by cargo from the newest release, the highest vMAJOR.MINOR.PATCH
# tag, never from main, into the build folder that `pigeon update` reuses,
# and keeping the program it replaces as pigeon.previous; then
# `pigeon setup`, which asks the rest. With `server --key <key> --member
# <name>`, it asks nothing and sets the machine up as an always-on server
# of the group: started at boot, following everything and keeping its
# history.
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
        echo "install gcc or clang"
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
            *) fail "unknown argument: $1" ;;
        esac
        shift
    done
    if [ "$mode" = server ] && { [ -z "$key" ] || [ -z "$member" ]; }; then
        fail "server needs --key and --member"
    fi
    case $(uname -s) in
        Linux) cache=${XDG_CACHE_HOME:-$HOME/.cache} ;;
        Darwin) cache=$HOME/Library/Caches ;;
        *) fail "install.sh installs pigeon on Linux and macOS" ;;
    esac
    repository=https://github.com/mariogeiger/pigeon
    shell_path=$PATH
    bin=${CARGO_HOME:-$HOME/.cargo}/bin
    PATH=$bin:$PATH

    command -v git >/dev/null 2>&1 || fail "git: install it from https://git-scm.com"
    command -v cc >/dev/null 2>&1 || fail "C compiler: $(compiler_fix)"
    if ! command -v cargo >/dev/null 2>&1; then
        if [ "$mode" = setup ]; then
            printf 'Rust is missing: install it with rustup? [Y/n] '
            read -r answer </dev/tty
            case $answer in
                "" | [yY]*) ;;
                *) fail "Rust : curl https://sh.rustup.rs -sSf | sh" ;;
            esac
        fi
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
            sh -s -- -y --profile minimal --no-modify-path </dev/null
    fi
    echo "✓ Rust  $(cargo --version)"

    source=$cache/pigeon/source
    stall="-c http.lowSpeedLimit=1000 -c http.lowSpeedTime=30"
    # shellcheck disable=SC2086
    listing=$(git $stall ls-remote --tags --refs "$repository" </dev/null) ||
        fail "git could not list the releases: check the connection to GitHub and try again"
    tag=$(printf '%s\n' "$listing" | sed -n 's|^.*refs/tags/\(v[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)$|\1|p' |
        sort -t. -k1.2,1n -k2,2n -k3,3n | tail -n 1)
    [ -n "$tag" ] || fail "pigeon has no release yet: $repository has no vMAJOR.MINOR.PATCH tag"
    echo "Fetching $tag from $repository"
    if [ -d "$source/.git" ]; then
        # shellcheck disable=SC2086
        git $stall -C "$source" fetch "$repository" "refs/tags/$tag:refs/tags/$tag" ||
            fail "git could not fetch pigeon: check the connection to GitHub and try again"
        git -c advice.detachedHead=false -C "$source" checkout --quiet --force --detach "$tag"
    else
        # shellcheck disable=SC2086
        git $stall -c advice.detachedHead=false clone --branch "$tag" "$repository" "$source" ||
            fail "git could not clone pigeon: check the connection to GitHub and try again"
    fi
    if [ -f "$bin/pigeon" ]; then
        cp -p "$bin/pigeon" "$bin/pigeon.previous"
    fi
    cargo install --locked --target-dir "$cache/pigeon/build" --path "$source/crates/pigeon" </dev/null
    echo "✓ pigeon built  $("$bin/pigeon" --version | sed 's/^pigeon //')"
    pigeon=$bin/pigeon
    if [ "$(PATH=$shell_path command -v pigeon || true)" = "$pigeon" ]; then
        echo "✓ Installed  $pigeon"
    else
        echo "✗ Installed  $bin is not on the PATH: . \"${bin%/bin}/env\""
    fi

    if [ "$mode" = server ]; then
        "$pigeon" service install --linger </dev/null
        "$pigeon" group join --key "$key" --member "$member" </dev/null
        "$pigeon" group serve </dev/null
        echo "✓ server of the group, as $member"
    else
        exec "$pigeon" setup </dev/tty
    fi
}

main "$@"
