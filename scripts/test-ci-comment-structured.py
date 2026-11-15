#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import os
import shutil
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('structured', Path(__file__).with_name('ci-comment-structured.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ShellComments(unittest.TestCase):
    @unittest.skipUnless(os.environ.get('SHFMT') or shutil.which('shfmt'), 'shfmt parser not installed')
    def test_shell_comments_with_expansions_and_multiline_literals(self):
        before = '# header\nname="$(printf \'value\')"\n# explanation\necho "${name}"\ncat <<EOF\n# literal\nEOF\n'
        after = before.replace('# header\n', '').replace('# explanation\n', '')
        self.assertEqual(module.compare(before, after, 'shell'), 'same')
        self.assertEqual(module.compare(before, before.replace('# literal', '# changed'), 'shell'), 'different')
        self.assertEqual(module.compare(before, before.replace('echo', 'eval'), 'shell'), 'different')

    def test_missing_parser_cannot_skip_runtime_checks(self):
        with patch.dict(os.environ, {'SHFMT': ''}), patch.object(module.shutil, 'which', return_value=None):
            self.assertEqual(module.compare('# first\necho hi', '# second\necho hi', 'shell'), 'needs_shfmt')

    def test_shebang_and_tool_directives_remain_significant(self):
        for before, after in [('#!/bin/bash\necho hi', '#!/bin/sh\necho hi'),
                              ('# shellcheck disable=SC2086\necho $x', '# note\necho $x')]:
            self.assertEqual(module.compare(before, after, 'shell'), 'different')


@unittest.skipUnless(shutil.which('ruby'), 'Ruby standard YAML parser not installed')
class YamlComments(unittest.TestCase):
    def test_block_scalar_contents_are_preserved(self):
        before = '# heading\njob:\n  run: |\n    # shell text\n    echo "ok"\n'
        self.assertEqual(module.compare(before, before.replace('# heading\n', ''), 'yaml'), 'same')
        self.assertEqual(module.compare(before, before.replace('# shell text', '# changed'), 'yaml'), 'different')
        self.assertEqual(module.compare(before, before.replace('echo', 'eval'), 'yaml'), 'different')

    def test_aliases_tags_and_quoted_values_remain_significant(self):
        before = '# header\nbase: &base [value]\nother: *base\nvalue: "# text"\n'
        self.assertEqual(module.compare(before, before.replace('# header\n', ''), 'yaml'), 'same')
        for changed in (before.replace('"# text"', '"# changed"'), before.replace('*base', '*missing'),
                        before.replace('value: "', 'value: !custom "')):
            self.assertNotEqual(module.compare(before, changed, 'yaml'), 'same')

    def test_invalid_yaml_never_selects_fast_path(self):
        self.assertEqual(module.compare('job: [\n', 'job: [\n', 'yaml'), 'unknown')


if __name__ == '__main__':
    unittest.main()
