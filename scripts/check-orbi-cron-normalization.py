#!/usr/bin/env python3
"""Exercise the collector's actual parser and rewrite with isolated fixtures."""
import pathlib
import subprocess
import tempfile
import time

config = pathlib.Path('src/syslog-ng/syslog-ng.conf').read_text()
start = config.index('filter f_routine_cron_launch')
end = config.index('template t_record')
rules = config[start:end]
commands = [
    'USER root pid 123 cmd /usr/sbin/orbi-agh-sync >/tmp/orbi-agh-sync.log 2>&1',
    'USER root pid 124 cmd /usr/sbin/orbi-discovery-snapshot >/dev/null 2>&1',
    'cannot execute job: Permission denied',
    'USER root pid 125 cmd /usr/sbin/other >/dev/null 2>&1',
]
cases = [(75, 'orbi-rbr20', 'crond[1504]', m, 'info' if i < 2 else 'err') for i, m in enumerate(commands)]
cases += [(75, 'other-host', 'crond[1504]', commands[0], 'err'),
          (27, 'orbi-rbr20', 'crond[1504]', commands[0], 'err'),
          (75, 'orbi-rbr20', 'other[1504]', commands[0], 'err'),
          (76, 'orbi-rbr20', 'crond[1504]', commands[0], 'warning')]
with tempfile.TemporaryDirectory() as temp:
    p = pathlib.Path(temp)
    (p/'input').write_text(''.join(f'<{pri}>Nov  1 18:12:00 {host} {app}: {msg}\n' for pri, host, app, msg, _ in cases))
    (p/'test.conf').write_text('@version: 3.38\noptions { keep-hostname(yes); flush-lines(1); };\n' + rules + '''
source s { file("/tmp/cron-check/input" flags(store-raw-message) follow-freq(1)); };
destination d { file("/tmp/cron-check/output" template("${LEVEL}|${RAWMSG}\\n")); };
log { source(s); rewrite(r_routine_cron_launch); destination(d); };
''')
    subprocess.run(['docker', 'compose', 'cp', temp+'/.', 'syslog-ng:/tmp/cron-check'], check=True, capture_output=True)
    subprocess.run(['docker', 'compose', 'exec', '-T', 'syslog-ng', 'rm', '-f', '/tmp/cron-check/output', '/tmp/cron-check/persist'], check=True)
    subprocess.run(['docker', 'compose', 'exec', '-T', 'syslog-ng', 'syslog-ng', '--no-caps', '-f', '/tmp/cron-check/test.conf', '--pidfile=/tmp/cron-check/pid', '--persist-file=/tmp/cron-check/persist', '--control=/tmp/cron-check/control'], check=True)
    try:
        time.sleep(2)
        output = subprocess.check_output(['docker', 'compose', 'exec', '-T', 'syslog-ng', 'cat', '/tmp/cron-check/output'], text=True).splitlines()
        assert len(output) == len(cases), output
        for line, (pri, host, app, msg, expected) in zip(output, cases):
            assert line == f'{expected}|<{pri}>Nov  1 18:12:00 {host} {app}: {msg}', line
        print(f'PASS: {len(cases)} cases; raw PRI preserved')
    finally:
        subprocess.run(['docker', 'compose', 'exec', '-T', 'syslog-ng', 'sh', '-c', 'kill "$(cat /tmp/cron-check/pid)"'], check=True)
