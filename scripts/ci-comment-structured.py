#!/usr/bin/env python3
"""Compare shell and YAML syntax without executing either input."""
import json
import os
import re
import shutil
import subprocess


DIRECTIVE = re.compile(r'(?i)^\s*#.*(?:^#!|shellcheck|shfmt|coding\s*[:=]|vim:|vi:|ex:)', re.M)
YAML_AST = r'''
require 'psych'
require 'json'
def syntax(value)
  case value
  when Psych::Nodes::Node
    [value.class.name, value.instance_variables.sort.filter_map do |key|
      next if [:@start_line, :@start_column, :@end_line, :@end_column].include?(key)
      [key.to_s, syntax(value.instance_variable_get(key))]
    end]
  when Array then value.map { |child| syntax(child) }
  else value
  end
end
puts JSON.generate(syntax(Psych.parse_stream(STDIN.read)))
'''


def compare(before, after, language):
    if language == 'shell':
        directives = lambda source: (source.splitlines()[:1] if source.startswith('#!') else [],
                                     DIRECTIVE.findall(source))
        if directives(before) != directives(after):
            return 'different'
        executable = os.environ.get('SHFMT') or shutil.which('shfmt')
        if not executable:
            return 'needs_shfmt'
        command = [executable, '-mn', '-ln', 'bash']
    elif language == 'yaml':
        executable = shutil.which('ruby')
        if not executable:
            return 'unknown'
        command = [executable, '-e', YAML_AST]
    else:
        return 'unknown'
    try:
        outputs = []
        for source in (before, after):
            result = subprocess.run(command, input=source, capture_output=True, text=True, timeout=15)
            if result.returncode:
                return 'unknown'
            outputs.append(result.stdout if language == 'shell' else json.loads(result.stdout))
        return 'same' if outputs[0] == outputs[1] else 'different'
    except (OSError, ValueError, subprocess.SubprocessError):
        return 'unknown'
