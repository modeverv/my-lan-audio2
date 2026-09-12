#!/usr/bin/env python3
"""Exact per-packet wait percentiles for alternating scheduling trials."""
import argparse,json,math
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('directory',type=Path)
a=p.parse_args()
results=json.loads((a.directory/'comparison.json').read_text())
summary=[]
for result in results:
    waits=[];warm=[];worst=[]
    for line in (a.directory/(result['run']+'.jsonl')).open():
        e=json.loads(line)
        if e['event']=='receive':
            if e['kernel_queue_ms']>=0:
                waits.append(e['kernel_queue_ms'])
                if e['receive_time_ns']>=10e9:warm.append(e['kernel_queue_ms'])
            worst.append(e)
    def stats(values):
        values.sort()
        if not values:return None
        return {'count':len(values),'p50':values[math.ceil(len(values)*.5)-1], 'p99':values[math.ceil(len(values)*.99)-1], 'p999':values[math.ceil(len(values)*.999)-1], 'max':max(values), 'over_1ms':sum(x>1 for x in values),'over_5ms':sum(x>5 for x in values),'over_10ms':sum(x>10 for x in values)}
    s=result['summary'];q=s['playout']
    summary.append({'run':result['run'],'seconds':s['elapsed_seconds'],'policy':result['policy'],'cpu_percent_one_core':result['cpu_percent_one_core'],'kernel_wait_ms':stats(waits),'kernel_wait_after_10s_ms':stats(warm),'handoff_batch_max_ms':s['handoff_batch_max_ms'],'callback_interval_ms':s['callback_interval_ms'],'callback_work_ms':s['callback_work_ms'],'received':s['received_packets'],'invalid':s['invalid_packets'],'queue_drops':s['queue_dropped'],'telemetry_drops':s['telemetry_dropped'],'late_packets':q['late_packets'],'missing_frames':q['missing_frames'],'underrun_callbacks':q['underruns'],'sessions':q['sessions'],'callback_discontinuities':q['callback_discontinuities'],'arrival_interval_ms':s['arrival_interval_ms'],'worst_interval_expansions':sorted(worst,key=lambda e:e['arrival_excess_ms'],reverse=True)[:3]})
print(json.dumps({'buffer_ms':results[0]['summary']['target_buffer_ms'],'callback_frames':results[0]['summary']['callback_frames'],'runs':summary,'notes':['Sequential Qos / Realtime / Realtime / Qos, same executable and source.','Kernel timestamps measure kernel timestamp to recvmsg completion, not a scheduler trace.','CPU includes receiver process and JSONL logging; 100% means one fully occupied core.','No guarantee of a hard deadline or long-term stability.']},indent=2))
