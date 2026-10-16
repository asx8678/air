import io
import os
import tempfile
import unittest
from contextlib import redirect_stdout

from todo import cli


class CliTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        os.environ["TODO_FILE"] = os.path.join(self.dir.name, "todo.json")

    def tearDown(self):
        self.dir.cleanup()
        del os.environ["TODO_FILE"]

    def run_cli(self, *argv):
        out = io.StringIO()
        with redirect_stdout(out):
            cli.main(list(argv))
        return out.getvalue()

    def test_add_and_list(self):
        self.run_cli("add", "buy milk")
        self.run_cli("add", "call mom")
        self.assertEqual(self.run_cli("list"), "1. [ ] buy milk\n2. [ ] call mom\n")

    def test_done(self):
        self.run_cli("add", "buy milk")
        self.run_cli("done", "1")
        self.assertEqual(self.run_cli("list"), "1. [x] buy milk\n")


if __name__ == "__main__":
    unittest.main()
