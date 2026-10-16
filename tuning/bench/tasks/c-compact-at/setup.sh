. "$(dirname "$0")/../../lib/setup.sh"
from_repo "$(git -C "$REPO" rev-parse --short b26214f^)"
