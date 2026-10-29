"""Rust lexical cases for the process-creation syntax gate."""
import importlib.util
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("creation_gate", ROOT / "scripts/check-process-creation.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class RustCreationLexerTests(unittest.TestCase):
    def test_creation_after_quote_character_is_visible(self):
        for call in ["cmd.spawn()", "cmd.status()", "std::process::Command::output(&mut cmd)"]:
            with self.subTest(call=call):
                source = "let q = '\"'; " + call + '; let s = "x";'
                self.assertEqual(sum(gate.creation_calls(source).values()), 1)

    def test_original_creation_negatives_are_visible(self):
        for call in ["cmd.spawn()", "cmd.status()", "cmd.output()"]:
            with self.subTest(call=call):
                self.assertEqual(sum(gate.creation_calls(call).values()), 1)

    def test_character_escape_and_byte_character_do_not_mask_code(self):
        for literal in [r"'\''", r"'\n'", r"'\x22'", r"'\u{22}'", "'中'", "b'\"'"]:
            with self.subTest(literal=literal):
                self.assertEqual(sum(gate.creation_calls(f"let q = {literal}; cmd.spawn();").values()), 1)

    def test_raw_strings_hide_only_their_contents(self):
        for literal in ['r"cmd.spawn()"', 'r#"quote " cmd.status()"#', 'br##"cmd.output(); "#"##', 'cr#"cmd.spawn()"#']:
            with self.subTest(literal=literal):
                self.assertEqual(sum(gate.creation_calls(f"let s = {literal}; cmd.spawn();").values()), 1)

    def test_nested_comments_and_strings_are_not_code(self):
        source = '/* outer /* cmd.spawn() */ cmd.output() */ "cmd.status()"; // cmd.spawn()\ncmd.output();'
        self.assertEqual(sum(gate.creation_calls(source).values()), 1)
        self.assertEqual(sum(gate.creation_calls('"escaped \\\" cmd.spawn()"').values()), 0)

    def test_lifetimes_and_labels_are_not_character_literals(self):
        for source in ["fn f<'a>(cmd: &'a mut Command) { cmd.spawn(); }", "'label: loop { cmd.status(); break 'label; }", "let value: &'_ str = \"x\"; Command::output(cmd);"]:
            with self.subTest(source=source):
                self.assertEqual(sum(gate.creation_calls(source).values()), 1)

    def test_multiline_calls_survive_literal_and_comment_masking(self):
        source = 'let q = \'"\'; cmd.\nspawn(/* comment " */);'
        self.assertEqual(sum(gate.creation_calls(source).values()), 1)

    def test_unterminated_literals_and_comments_fail_closed(self):
        for source in ['/* nested /* end */', 'r##"never closes"#', '"never closes']:
            with self.subTest(source=source):
                with self.assertRaises(ValueError): gate.creation_calls(source)

    def test_mask_preserves_offsets_and_lines(self):
        source = "'\"'; r#\"one\ntwo\"#; /* three\n */ cmd.spawn();"
        masked = gate.mask_rust(source)
        self.assertEqual(len(source), len(masked))
        self.assertEqual([i for i, c in enumerate(source) if c == '\n'], [i for i, c in enumerate(masked) if c == '\n'])


if __name__ == "__main__": unittest.main(verbosity=2)
