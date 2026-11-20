#!/usr/bin/env bash
# Build a toy git repo + one linked worktree under the directory given as $1.
# Layout: $1/fixture-repo (main checkout), $1/fixture-wt (linked worktree).
set -euo pipefail
root=$1
mkdir -p "$root/fixture-repo" && cd "$root/fixture-repo"
# Inherited GIT_* variables (GIT_DIR beats `-C`/cd) would make git act on a real
# repository: drop them all, then isolate config (AGENTS.md "Test isolation").
for v in $(env | grep -o '^GIT_[A-Za-z0-9_]*'); do unset "$v"; done
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
git init -q -b main .
git config user.name Fixture; git config user.email fixture@example.invalid
mkdir -p src tests
cat > src/calc.py <<'PY'
def add(a, b):
    return a + b
PY
cat > tests/test_calc.py <<'PY'
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))
from calc import add


class AddTest(unittest.TestCase):
    def test_add(self):
        self.assertEqual(add(1, 2), 3)


if __name__ == "__main__":
    unittest.main()
PY
printf '# fixture\n' > README.md
git add -A && git commit -q -m "initial"
git worktree add -q -b feature/wt ../fixture-wt
