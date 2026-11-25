#!/usr/bin/env python3
"""Median of three independent run medians; no pooled percentile claim."""
import json, statistics, sys
from collections import defaultdict
from pathlib import Path
rows=[json.loads(line) for line in Path(sys.argv[1]).read_text().splitlines()]
groups=defaultdict(list)
for row in rows:
    assert row['verified'] is True
    groups[(row['rows'],row['backend'])].append(row)
assert len(rows)==18 and len(groups)==6
out=[]
for (n,backend),rs in sorted(groups.items()):
    assert sorted(r['rep'] for r in rs)==[0,1,2]
    s={'rows':n,'backend':backend,'runs':3}
    for metric in ('insert','update','read','replay','reopen'):
        s[metric+'_median_ms']=statistics.median(r[metric]['median_ms'] for r in rs)
        s[metric+'_p95_ms_median_of_runs']=statistics.median(r[metric]['p95_ms'] for r in rs)
        s[metric+'_run_median_range_ms']=[min(r[metric]['median_ms'] for r in rs),max(r[metric]['median_ms'] for r in rs)]
    s['insert_tx_per_s']=statistics.median(r['insert']['n']/(r['insert']['sum_ms']/1000) for r in rs)
    s['update_tx_per_s']=statistics.median(r['update']['n']/(r['update']['sum_ms']/1000) for r in rs)
    for metric in ('create_ms','live_bytes','closed_bytes'):
        s[metric]=statistics.median(r[metric] for r in rs)
    out.append(s)
print(json.dumps(out,indent=2))
