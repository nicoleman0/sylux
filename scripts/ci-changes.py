#!/usr/bin/env python3
"""Classify changes conservatively. Unknown input keeps every verification gate."""
import argparse
import json
import os
from pathlib import Path
import re
import runpy
import subprocess
import tempfile


STRUCTURED = runpy.run_path(str(Path(__file__).with_name('ci-comment-structured.py')))['compare']

MODEL = 'verification/account-authority/'
MODEL_RUNNERS = {'scripts/check-account-authority.py', 'scripts/test-account-authority-check.py',
                 'verification/tlc.json'}
RECOVERY_DOCS = {'docs/docker-beta.md', 'docs/lxc.md'}
LANGUAGES = {'.rs': 'rust', '.ts': 'js', '.tsx': 'js', '.js': 'js', '.jsx': 'js',
             '.mjs': 'js', '.cjs': 'js', '.css': 'css', '.sql': 'sql', '.py': 'python',
             '.sh': 'shell', '.bash': 'shell', '.yaml': 'yaml', '.yml': 'yaml',
             '.toml': 'toml', '.svelte': 'svelte', '.html': 'html'}
DIRECTIVE = re.compile(
    r'(?i)(@|eslint|tslint|webpack|rollup|__PURE__|__NO_SIDE_EFFECTS__|<reference|svelte-ignore|shellcheck|'
    r'sourceMappingURL|sourceURL|prettier|biome|rustfmt|clippy|istanbul|\bc8\b|'
    r'noqa|nosec|pylint|mypy|pyright|yapf|type\s*:|coding\s*[:=]|'
    r'^\s*[#!@]|^\s*(syntax|escape|check)\s*=)')


def opaque(value):
    # NUL cannot occur in accepted source, so literals cannot collide with code.
    return '\0' + json.dumps(value, ensure_ascii=True) + '\0'


def canonical(source, language):
    """Return code and Rust docs, or None when the small lexer cannot prove safety."""
    if '\0' in source or any(ord(c) < 32 and c not in '\n\r\t' for c in source):
        return None
    if language == 'docker':
        if '<<' in source:
            return None
        lines = []
        for line in source.splitlines():
            if line.lstrip().startswith('#'):
                if DIRECTIVE.search(line.lstrip()[1:]):
                    lines.append(opaque(line))
            elif line.strip():
                lines.append(line.rstrip())
        return '\n'.join(lines), []
    if language == 'shell' and any(token in source for token in ('<<', '${', '$(', '`', '\\')):
        return None
    if language in {'html', 'svelte'} and re.search(r'<(?:script|style|textarea|title)\b', source, re.I):
        return None
    if language in {'shell', 'python'} and re.search(r'\\\r?\n', source):
        return None
    if language == 'yaml' and any(token in source for token in ('|', '>')):
        return None
    if language == 'css' and (re.search(r'url\s*\(', source, re.I) or '\\' in source):
        return None
    out, docs = [], []
    in_tag = False
    i = 0
    while i < len(source):
        tail = source[i:]
        if language == 'js' and source[i] in '`<':
            return None
        if language == 'rust':
            raw = re.match(r'(?:br|r)(#*)"', tail)
            if raw:
                end = source.find('"' + raw[1], i + raw.end())
                if end < 0:
                    return None
                end += 1 + len(raw[1])
                out.append(opaque(source[i:end]))
                i = end
                continue
        # Treat Python interpolation as ambiguous, including nested same-quote strings.
        if language == 'python' and re.match(r'(?i)(?:f|fr|rf)[\'\"]', tail):
            return None
        quote = source[i]
        if language == 'sql' and quote in '[`$':
            return None
        if quote in '\'"':
            if language == 'rust' and quote == "'":
                char = re.match(r"'(?:[^'\\\n]|\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.))'", tail)
                if not char:
                    out.append(quote)
                    i += 1
                    continue
            delimiter = quote
            if language in {'python', 'toml'} and tail.startswith(quote * 3):
                delimiter *= 3
            j = i + len(delimiter)
            while j < len(source):
                if source.startswith(delimiter, j):
                    j += len(delimiter)
                    break
                if source[j] == '\\' and not (language in {'shell', 'toml', 'yaml', 'sql'} and quote == "'"):
                    j += 2
                else:
                    j += 1
            else:
                return None
            if j > len(source):
                return None
            out.append(opaque(source[i:j]))
            i = j
            continue
        start, close = None, None
        if language in {'rust', 'js', 'css'} and tail.startswith('/*'):
            start, close = '/*', '*/'
        elif language in {'rust', 'js'} and tail.startswith('//'):
            start, close = '//', '\n'
        elif language == 'sql' and tail.startswith('--'):
            start, close = '--', '\n'
        elif language == 'sql' and tail.startswith('/*'):
            start, close = '/*', '*/'
        elif language in {'python', 'shell', 'yaml', 'toml'} and quote == '#':
            if language not in {'shell', 'yaml'} or i == 0 or source[i - 1].isspace():
                start, close = '#', '\n'
        elif language in {'html', 'svelte'} and not in_tag and tail.startswith('<!--'):
            start, close = '<!--', '-->'
        if start:
            j = i + len(start)
            if close == '\n':
                end = source.find('\n', j)
                end = len(source) if end < 0 else end
            elif language == 'rust':
                depth = 1
                while j < len(source) and depth:
                    if source.startswith('/*', j):
                        depth += 1
                        j += 2
                    elif source.startswith('*/', j):
                        depth -= 1
                        j += 2
                    else:
                        j += 1
                if depth:
                    return None
                end = j
            else:
                end = source.find(close, j)
                if end < 0:
                    return None
                end += len(close)
                if language == 'sql' and '/*' in source[j:end - len(close)]:
                    return None
            comment = source[i:end]
            rustdoc = language == 'rust' and (
                (comment.startswith('///') and not comment.startswith('////')) or
                comment.startswith('//!') or comment.startswith('/*!') or
                (comment.startswith('/**') and not comment.startswith('/***')))
            if rustdoc:
                docs.append(comment)
            if DIRECTIVE.search(comment[len(start):]) and not rustdoc:
                out.append(opaque(comment))
            else:
                prefix = source[source.rfind('\n', 0, i) + 1:i]
                next_line = source.find('\n', end)
                suffix = source[end:len(source) if next_line < 0 else next_line]
                inline = close != '\n' and (prefix.strip() or suffix.strip())
                out.append((opaque('block comment') if inline else ' ') + '\n' * comment.count('\n'))
            i = end
            continue
        # A slash may start a regex. A markup expression may contain arbitrary JS.
        if language == 'js' and quote == '/':
            return None
        if language in {'html', 'svelte'} and quote in '{`':
            return None
        if language in {'html', 'svelte'}:
            if quote == '<':
                in_tag = True
            elif quote == '>':
                in_tag = False
        out.append(quote)
        i += 1
    code = '\n'.join(line.rstrip() for line in ''.join(out).splitlines() if line.strip())
    return code, docs


def git(*args):
    result = subprocess.run(['git', *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise ValueError('Git comparison failed; fetch the base commit and rerun classification.')
    return result.stdout


def blob(commit, path):
    content = git('show', f'{commit}:{path}')
    if b'\0' in content:
        raise ValueError(f'Binary input: {path}')
    try:
        return content.decode('utf-8')
    except UnicodeDecodeError as error:
        raise ValueError(f'Non-UTF-8 input: {path}') from error


def node_equivalent(path, before, after):
    with tempfile.TemporaryDirectory(prefix='sylux-ci-syntax-') as directory:
        before_path = Path(directory) / 'before'
        after_path = Path(directory) / 'after'
        before_path.write_text(before, encoding='utf-8')
        after_path.write_text(after, encoding='utf-8')
        try:
            result = subprocess.run(
                ['node', str(Path(__file__).with_name('ci-comment-syntax.mjs')),
                 path, str(before_path), str(after_path)],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
            return result.returncode if result.returncode in {0, 2} else 1
        except FileNotFoundError:
            return 2
        except subprocess.TimeoutExpired:
            return 1


def lightweight(path):
    name = Path(path).name.lower()
    return Path(path).suffix.lower() == '.md' or name in {
        'license', 'licence', 'license.txt', 'licence.txt', 'copying', 'notice', 'notice.txt'}


def classify(base, head):
    result = dict(full=False, authority=False, recovery=False, rustdoc=False, needs_node=False, needs_shfmt=False)
    reasons = []
    try:
        base = git('rev-parse', '--verify', base + '^{commit}').decode().strip()
        head = git('rev-parse', '--verify', head + '^{commit}').decode().strip()
        tracked = set()
        for commit in {base, head}:
            coverage = json.loads(blob(commit, MODEL + 'coverage.json'))
            if not isinstance(coverage, dict) or not isinstance(coverage.get('files'), dict):
                raise ValueError('Model coverage metadata needs a files mapping.')
            tracked.update(coverage['files'])
        fields = git('diff', '--raw', '--no-abbrev', '-z', '--find-renames', base, head).decode().split('\0')
        i = 0
        while i < len(fields) - 1:
            metadata, path = fields[i:i + 2]
            old_mode, new_mode, _, _, status = metadata[1:].split()
            i += 2
            old_path = path
            if status.startswith(('R', 'C')):
                path = fields[i]
                i += 1
            if path in tracked or old_path in tracked:
                result['authority'] = True
            # Read every present blob before the path fast paths, including documentation.
            before = blob(base, old_path) if not status.startswith('A') else None
            after = blob(head, path) if not status.startswith('D') else None
            if ((old_mode != new_mode and not status.startswith(('A', 'D'))) or
                    old_mode not in {'100644', '100755', '000000'} or
                    new_mode not in {'100644', '100755', '000000'}):
                result.update(full=True, authority=True, recovery=True)
                reasons.append(f'{path}: file type or execution permissions changed')
                continue
            if ((path.startswith(MODEL) or path in MODEL_RUNNERS) and
                    (old_path.startswith(MODEL) or old_path in MODEL_RUNNERS)):
                result['authority'] = True
                reasons.append('account authority verification changed')
                continue
            if lightweight(path) and lightweight(old_path):
                result['recovery'] |= path in RECOVERY_DOCS or old_path in RECOVERY_DOCS
                reasons.append('documentation changed')
                continue
            language = LANGUAGES.get(Path(path).suffix.lower())
            if Path(path).name == 'Dockerfile':
                language = 'docker'
            if status == 'M' and language:
                prior = canonical(before, language)
                current = canonical(after, language)
                if prior is not None and current is not None and prior[0] == current[0]:
                    result['rustdoc'] |= prior[1] != current[1]
                    reasons.append('comments changed')
                    continue
                if language in {'shell', 'yaml'} and (prior is None or current is None):
                    parsed = STRUCTURED(before, after, language)
                    if parsed == 'same':
                        reasons.append('parser verified comment-only changes')
                        continue
                    if parsed == 'needs_shfmt':
                        result['needs_shfmt'] = True
                        reasons.append(f'{path}: install pinned shfmt and rerun classification')
                if language in {'js', 'svelte'}:
                    parsed = node_equivalent(path, before, after)
                    if parsed == 0:
                        reasons.append('parser verified comment-only changes')
                        continue
                    if parsed == 2:
                        result['needs_node'] = True
                        reasons.append(f'{path}: install pinned web dependencies and rerun classification')
            result.update(full=True, authority=True, recovery=True)
            reasons.append(f'{path}: executable/configuration change or unsupported syntax')
    except (ValueError, OSError, IndexError, TypeError) as error:
        result.update(full=True, authority=True, recovery=True)
        reasons.append(str(error))
    result['reason'] = '; '.join(dict.fromkeys(reasons)) or 'no changed files'
    return {key: str(value).lower() if isinstance(value, bool) else value for key, value in result.items()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', required=True)
    parser.add_argument('--head', default='HEAD')
    args = parser.parse_args()
    result = classify(args.base, args.head)
    output = os.environ.get('GITHUB_OUTPUT')
    if output:
        with open(output, 'a', encoding='utf-8') as stream:
            for key, value in result.items():
                stream.write(f'{key}={value.replace(chr(10), " ").replace(chr(13), " ")}\n')
    print(json.dumps(result, sort_keys=True))


if __name__ == '__main__':
    main()
