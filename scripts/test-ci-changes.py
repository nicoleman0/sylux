#!/usr/bin/env python3
"""Exercise classification against committed blobs, not the working tree."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name('ci-changes.py').resolve()
SPEC = importlib.util.spec_from_file_location('ci_changes', SCRIPT)
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)


class LexerTests(unittest.TestCase):
    def same(self, language, before, after):
        left, right = CI.canonical(before, language), CI.canonical(after, language)
        self.assertIsNotNone(left)
        self.assertIsNotNone(right)
        self.assertEqual(left[0], right[0])

    def different(self, language, before, after):
        left, right = CI.canonical(before, language), CI.canonical(after, language)
        self.assertTrue(left is None or right is None or left[0] != right[0])

    def test_ordinary_comments(self):
        for language, before, after in [
            ('rust', '// old\nfn a() {}\n', '// new\n// extra\nfn a() {}\n'),
            ('rust', 'fn a() { /* old */ 1 }', 'fn a() { /* new */ 1 }'),
            ('rust', '/* one /* nested */ */\nfn a() {}', '/* new */\nfn a() {}'),
            ('js', 'const x = "// literal"; // old', 'const x = "// literal"; // new'),
            ('css', '/* old */\na { color: red; }', '/* new */\na { color: red; }'),
            ('sql', 'SELECT \'-- literal\'; -- old', 'SELECT \'-- literal\'; -- new'),
            ('sql', "SELECT '$argon2id$*', '[a-z]*'; -- old", "SELECT '$argon2id$*', '[a-z]*'; -- new"),
            ('python', 'x = "# literal" # old', 'x = "# literal" # new'),
            ('shell', 'echo "# literal" # old', 'echo "# literal" # new'),
            ('yaml', 'key: "# literal" # old', 'key: "# literal" # new'),
            ('toml', 'key = "# literal" # old', 'key = "# literal" # new'),
        ]:
            with self.subTest(language=language, before=before):
                self.same(language, before, after)

    def test_docker_comments_and_continuations(self):
        self.same('docker', '# old\nFROM image\nRUN echo \\\n  hi\n',
                  '# new\nFROM image\nRUN echo \\\n  hi\n')
        for directive in ('syntax', 'escape', 'check'):
            self.different('docker', f'# {directive}=old\nFROM image',
                           f'# {directive}=new\nFROM image')
        self.assertIsNone(CI.canonical('RUN cat <<EOF\n# data\nEOF', 'docker'))

    def test_multiline_literal_content_is_never_a_comment(self):
        for language, source in [
            ('rust', 'let x = r###"line\n// old\n/* old */\n"###;'),
            ('rust', 'let x = "line\n// old\n";'),
            ('python', 'x = """line\n# old\n"""'),
            ('toml', 'x = """line\n# old\n"""'),
            ('js', 'const x = `line\n// old\n`;'),
            ('shell', 'cat <<EOF\n# old\nEOF'),
            ('yaml', 'key: |\n  # old\n'),
            ('yaml', 'key: &anchor |\n  # old\n'),
            ('css', 'a { background: url(https://a/*old*/); }'),
            ('css', r'a { value: \/*old*/; }'),
            ('html', '<p attr=<!--old-->>Hello</p>'),
            ('html', '<textarea><!-- old --></textarea>'),
            ('sql', 'SELECT [-- old];'),
            ('js', 'const x = /[/*old*/]/;'),
            ('js', 'const x = <p>// old</p>;'),
        ]:
            with self.subTest(language=language):
                self.different(language, source, source.replace('old', 'new'))

    def test_sql_dialect_escapes_cannot_hide_changed_data(self):
        before = "SELECT E'it\\'s -- old';"
        self.different('sql', before, before.replace('old', 'new'))
        before = "SELECT q'{it's -- old}';"
        self.different('sql', before, before.replace('old', 'new'))

    def test_markup_whitespace_needs_a_real_parser(self):
        before = '<pre>\n\n<!-- old -->\ntext</pre>'
        after = '<pre>\n<!-- new -->\ntext</pre>'
        self.different('html', before, after)
        self.different('svelte', before, after)

    def test_literal_whitespace_is_preserved(self):
        self.different('python', 'x = """a\n \n"""', 'x = """a\n\n"""')
        self.different('rust', 'let x = r#"a  \n"#;', 'let x = r#"a\n"#;')

    def test_yaml_hash_inside_plain_scalar(self):
        self.different('yaml', 'key: value#old', 'key: value#new')

    def test_block_comment_presence_preserves_token_boundaries(self):
        self.different('css', '.a/**/.b {}', '.a .b {}')
        self.different('rust', 'foo/* old */bar', 'foobar')
        self.different('js', 'return /* old */ foo', 'return /*\n new */ foo')

    def test_directives_are_significant(self):
        for language, before, after in [
            ('js', '// @ts-check\nconst x = 1;', '// comment\nconst x = 1;'),
            ('js', '// eslint-disable no-eval\nx();', '// eslint-enable no-eval\nx();'),
            ('js', 'x(); //# sourceMappingURL=a', 'x(); //# sourceMappingURL=b'),
            ('svelte', '<!-- svelte-ignore a -->', '<!-- svelte-ignore b -->'),
            ('shell', '# shellcheck disable=SC1\necho x', '# shellcheck disable=SC2\necho x'),
            ('shell', '#!/bin/sh\necho x', '#!/bin/bash\necho x'),
            ('python', '# coding: utf-8\nx=1', '# coding: ascii\nx=1'),
            ('python', 'x=1 # type: ignore', 'x=1 # comment'),
            ('rust', '// rustfmt::skip\nfn a() {}', '// comment\nfn a() {}'),
        ]:
            with self.subTest(language=language, before=before):
                self.different(language, before, after)

    def test_rust_docs_are_separate(self):
        before = CI.canonical('/// old\nfn a() {}', 'rust')
        after = CI.canonical('/// new\nfn a() {}', 'rust')
        self.assertEqual(before[0], after[0])
        self.assertNotEqual(before[1], after[1])

    def test_lifetimes_and_chars(self):
        self.same('rust', "fn f<'a>(x: &'a str) { let c = '/'; } // old",
                  "fn f<'a>(x: &'a str) { let c = '/'; } // new")

    def test_ambiguous_input_fails_closed(self):
        for language, source in [('rust', '/* unclosed'), ('python', 'x="unclosed'),
                                 ('python', 'x = f"{a["b"]}"'), ('shell', 'echo ${x#old}'),
                                 ('shell', 'echo `cat file`'), ('python', 'x=1\\\n # old'),
                                 ('rust', 'let x = 1;\0'), ('sql', '/* nested /* old */ outer */ SELECT 1')]:
            with self.subTest(language=language, source=source):
                self.assertIsNone(CI.canonical(source, language))


class RepositoryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git('init', '-q')
        self.git('config', 'user.name', 'CI Classifier Test')
        self.git('config', 'user.email', 'classifier@example.invalid')
        self.write('verification/account-authority/coverage.json', json.dumps({'files': {'src/tracked.rs': 'hash'}}))
        self.write('src/tracked.rs', '// old\nfn tracked() {}\n')
        self.write('src/main.rs', '// old\nfn main() {}\n')
        self.write('README.md', 'old\n')
        self.write('docs/lxc.md', 'old\n')
        self.write('scripts/check-account-authority.py', 'print("old")\n')
        self.write('.github/workflows/ci.yml', '# old\nname: CI\n')
        self.write('scripts/run.sh', '# old\necho hello\n')
        self.git('add', '.')
        self.git('commit', '-qm', 'base')
        self.base = self.git('rev-parse', 'HEAD').strip()

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.root, text=True)

    def write(self, path, content):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content if isinstance(content, bytes) else content.encode())

    def classify(self, changes=None, base=None, env=None):
        for path, content in (changes or {}).items():
            if content is None:
                (self.root / path).unlink()
            else:
                self.write(path, content)
        self.git('add', '-A')
        self.git('commit', '-qm', 'change', '--allow-empty')
        output = self.root / 'outputs'
        process = subprocess.run([sys.executable, str(SCRIPT), '--base', base or self.base],
                                 cwd=self.root, env={**os.environ, 'GITHUB_OUTPUT': str(output), **(env or {})},
                                 text=True, capture_output=True, check=True)
        result = json.loads(process.stdout)
        emitted = dict(line.split('=', 1) for line in output.read_text().splitlines())
        self.assertEqual(result, emitted)
        return result

    def flags(self, result, *, full=False, authority=False, recovery=False, rustdoc=False, needs_node=False, needs_shfmt=False):
        self.assertEqual({key: result[key] for key in ('full', 'authority', 'recovery', 'rustdoc', 'needs_node', 'needs_shfmt')},
                         {key: str(value).lower() for key, value in locals().items()
                          if key in {'full', 'authority', 'recovery', 'rustdoc', 'needs_node', 'needs_shfmt'}})
        self.assertTrue(result['reason'])

    def test_no_changes(self):
        self.flags(self.classify())

    def test_docs_additions_and_deletions(self):
        self.flags(self.classify({'README.md': None, '.agents/skills/new/SKILL.md': 'new',
                                  'LICENSE': 'new'}))

    def test_recovery_doc(self):
        self.flags(self.classify({'docs/lxc.md': 'updated install guide'}), recovery=True)

    def test_ordinary_rust_comment(self):
        self.flags(self.classify({'src/main.rs': '// new\nfn main() {}\n'}))

    def test_rust_doc(self):
        self.flags(self.classify({'src/main.rs': '/// new\nfn main() {}\n'}), rustdoc=True)

    def test_tracked_comment_keeps_authority_guard(self):
        self.flags(self.classify({'src/tracked.rs': '// new\nfn tracked() {}\n'}), authority=True)

    def test_tracked_doc_keeps_authority_guard(self):
        self.flags(self.classify({'src/tracked.rs': '/// new\nfn tracked() {}\n'}),
                   authority=True, rustdoc=True)

    def test_model_metadata_only(self):
        self.flags(self.classify({'verification/account-authority/coverage.json':
                                  json.dumps({'files': {'src/tracked.rs': 'new hash'}, 'sync_commit': 'new'})}),
                   authority=True)

    def test_model_runner_executable_change(self):
        self.flags(self.classify({'scripts/check-account-authority.py': 'print("new")\n'}), authority=True)

    def test_executable_change(self):
        self.flags(self.classify({'src/main.rs': 'fn main() { panic!("changed"); }\n'}),
                   full=True, authority=True, recovery=True)

    def test_sql_fast_path_is_limited_to_the_owned_sqlite_dialect(self):
        self.write('external/query.sql', "SELECT q'{it's -- old}';")
        self.git('add', '.')
        self.git('commit', '-qm', 'unknown SQL dialect')
        self.base = self.git('rev-parse', 'HEAD').strip()
        self.flags(self.classify({'external/query.sql': "SELECT q'{it's -- new}';"}),
                   full=True, authority=True, recovery=True)

    def test_deleted_source(self):
        self.flags(self.classify({'src/main.rs': None}), full=True, authority=True, recovery=True)

    def test_renamed_source(self):
        self.git('mv', 'src/main.rs', 'src/renamed.rs')
        self.flags(self.classify(), full=True, authority=True, recovery=True)

    def test_renamed_docs(self):
        self.git('mv', 'README.md', 'ABOUT.md')
        self.flags(self.classify())

    def test_source_renamed_into_model_is_not_model_only(self):
        self.git('mv', 'src/main.rs', 'verification/account-authority/main.rs')
        self.flags(self.classify(), full=True, authority=True, recovery=True)

    def test_unknown_path(self):
        self.flags(self.classify({'data.example': 'unknown'}), full=True, authority=True, recovery=True)

    def test_binary_docs(self):
        self.flags(self.classify({'README.md': b'a\0b'}), full=True, authority=True, recovery=True)

    def test_missing_base(self):
        result = self.classify(base='missing-base')
        self.flags(result, full=True, authority=True, recovery=True)
        self.assertIn('fetch the base commit', result['reason'])

    def test_workflow_comment_only(self):
        self.flags(self.classify({'.github/workflows/ci.yml': '# new\nname: CI\n'}))

    def test_workflow_config(self):
        self.flags(self.classify({'.github/workflows/ci.yml': 'name: changed\n'}),
                   full=True, authority=True, recovery=True)

    def test_classifier_policy_change(self):
        self.flags(self.classify({'scripts/ci-changes.py': 'print("changed policy")\n'}),
                   full=True, authority=True, recovery=True)

    def test_missing_node_dependencies_fail_closed(self):
        self.write('web/example.ts', 'const x = `hello`; // old')
        self.git('add', '.')
        self.git('commit', '-qm', 'parser input')
        self.base = self.git('rev-parse', 'HEAD').strip()
        self.write('bin/node', '#!/bin/sh\nexit 2\n')
        (self.root / 'bin/node').chmod(0o755)
        self.flags(self.classify({'web/example.ts': 'const x = `hello`; // new'},
                                 env={'PATH': str(self.root / 'bin') + os.pathsep + os.environ['PATH']}),
                   full=True, authority=True, recovery=True, needs_node=True)

    @unittest.skipUnless(__import__('shutil').which('ruby'), 'Ruby YAML parser unavailable')
    def test_workflow_header_comments_with_block_scalars(self):
        source = '# first\nname: CI\njobs:\n  check:\n    run: |\n      # shell text\n      echo hello\n'
        self.write('.github/workflows/ci.yml', source)
        self.git('add', '.')
        self.git('commit', '-qm', 'workflow fixture')
        self.base = self.git('rev-parse', 'HEAD').strip()
        self.flags(self.classify({'.github/workflows/ci.yml': source.replace('# first', '# next')}))

    def test_execution_permissions(self):
        (self.root / 'scripts/run.sh').chmod(0o755)
        self.flags(self.classify(), full=True, authority=True, recovery=True)

    def test_symlink_source(self):
        (self.root / 'src/main.rs').unlink()
        (self.root / 'src/main.rs').symlink_to('tracked.rs')
        self.flags(self.classify(), full=True, authority=True, recovery=True)

    def test_bad_model_metadata(self):
        self.flags(self.classify({'verification/account-authority/coverage.json': '{broken'}),
                   full=True, authority=True, recovery=True)


if __name__ == '__main__':
    unittest.main()
