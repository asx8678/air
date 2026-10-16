# Sourced by each task's setup.sh. $1 = the repo to create; BENCH = tuning/bench; AIR = the air repo.
set -eu
DEST=$1
BENCH=$(cd "$(dirname "$0")/../.." && pwd)
AIR=$(cd "$BENCH/../.." && pwd)

# A fresh repo with one commit: the agent sees no history, so it can't look up the answer.
commit_all() {
    git -C "$DEST" init -q -b main
    git -C "$DEST" -c user.name=bench -c user.email=bench@localhost add -A
    git -C "$DEST" -c user.name=bench -c user.email=bench@localhost commit -q -m "task start"
}

from_template() {
    mkdir -p "$DEST"
    cp -R "$BENCH/tasks/$1/template/." "$DEST/"
    commit_all
}

# The air tree at a commit, without its history; a warm target/ from the prepared cache when there is one.
from_air() {
    mkdir -p "$DEST"
    git -C "$AIR" archive "$1" | tar -x -C "$DEST"
    commit_all
    cache="${AIR_BENCH_CACHE:-${TMPDIR:-/tmp}/air-bench/cache}/$1/target"
    if [ -d "$cache" ]; then cp -cR "$cache" "$DEST/target" 2>/dev/null || cp -R "$cache" "$DEST/target"; fi
}
