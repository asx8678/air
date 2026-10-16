. "$(dirname "$0")/../../lib/setup.sh"
from_repo "$(git -C "$REPO" rev-parse --short 5050a96^)"
