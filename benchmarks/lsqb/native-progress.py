#!/usr/bin/env python3
"""Summarize retained native Neo4j progress; never infer process liveness."""
import argparse
from collections import Counter
import json
from pathlib import Path


def summarize(directory):
    invocation = json.loads((directory / 'invocation.json').read_text())
    command = invocation['command']
    index = command.index('qualify')
    scale = command[index + 3]
    warmups, runs = (map(int, command[-2:]) if len(command) == index + 7 else (0, 1))
    contract = json.loads((Path(__file__).with_name('evidence-manifest-v2.json')).read_text())
    target = contract['datasets'][scale]
    journal = directory / 'result/observations.jsonl'
    counts, phases = Counter(), Counter()
    loaded = {'nodes': 0, 'edges': 0}
    current = None
    ignored_partial_line = False
    if journal.exists():
        raw = journal.read_bytes()
        lines = raw.splitlines(keepends=True)
        for line in lines:
            if not line.endswith(b'\n'):
                ignored_partial_line = True
                continue
            event = json.loads(line)
            if event.get('event') == 'load-progress':
                loaded = {key: event[key] for key in ('nodes', 'edges')}
            elif event.get('event') == 'query-start':
                current = {key: event.get(key) for key in ('suite', 'id', 'phase', 'sample_index')}
            elif event.get('event') == 'observation-recorded':
                counts[event['outcome']] += 1
                phases[event.get('phase', 'measurement')] += 1
                current = None
    total = 22 * (warmups + runs)
    completed = sum(counts.values())
    # A finished report is distinct from a finished supervisor or successful audit.
    report_present = (directory / 'result/diagnostic.json').is_file()
    return dict(event='native-progress-snapshot', scale=scale,
                process_liveness='not-checked', publication_qualified=False,
                loaded=loaded, load_target={key: target[key] for key in ('nodes', 'edges')},
                load_percent={key: round(100 * loaded[key] / target[key], 2)
                              for key in ('nodes', 'edges')},
                samples_recorded=completed, samples_expected=total,
                outcomes=dict(counts), phases=dict(phases), current_query=current,
                diagnostic_report_present=report_present,
                ignored_partial_line=ignored_partial_line)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    print(json.dumps(summarize(args.directory), sort_keys=True))
