#!/usr/bin/env python3
"""Alternating real-LAN scheduling comparison. Requires sender running; stops each child cleanly."""
import argparse,json,math,resource,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--source',required=True);p.add_argument('--seconds',type=float,default=90)
p.add_argument('--frames',type=int,default=128)
p.add_argument('--modes',default='qos,realtime,realtime,qos')
p.add_argument('--output-dir',type=Path,required=True);p.add_argument('--buffer-ms',type=float,default=5)
a=p.parse_args()
if not math.isfinite(a.seconds) or not 1 <= a.seconds <= 3600: p.error("seconds must be 1..3600")
if not math.isfinite(a.buffer_ms) or not 2 <= a.buffer_ms <= 100: p.error("buffer-ms must be 2..100")
if a.frames not in (64,128,256,512): p.error("frames must be 64,128,256,512")
a.output_dir.mkdir(parents=True,exist_ok=True)
root=Path(__file__).resolve().parents[1]
results=[]
for index,mode in enumerate(a.modes.split(','),1):
    if mode not in ('qos','realtime'):raise ValueError('unknown scheduling mode')
    label=f'{mode}-{index}'
    path=a.output_dir/(label+'.jsonl')
    before=resource.getrusage(resource.RUSAGE_CHILDREN)
    with (a.output_dir/(label+'-console.jsonl')).open('x') as output:
        child=subprocess.Popen([str(root/'target/release/receiver-macos'),'--source',a.source,'--buffer-ms',str(a.buffer_ms),'--buffer-frames',str(a.frames),'--network-scheduling',mode,'--seconds',str(a.seconds),'--events','--output',str(path)],stdout=output)
        try:code=child.wait()
        except BaseException:
            child.send_signal(2);child.wait(timeout=5);raise
    after=resource.getrusage(resource.RUSAGE_CHILDREN)
    if code:raise RuntimeError(f'{label} exited {code}')
    final=None;policy=None
    for line in path.open():
        row=json.loads(line)
        if row['event']=='receiver_scheduling':policy=row
        if row['event']=='receiver_final':final=row
    if final is None:raise RuntimeError('missing final summary')
    elapsed=final['elapsed_seconds']
    row={'run':label,'cpu_percent_one_core':100*((after.ru_utime+after.ru_stime)-(before.ru_utime+before.ru_stime))/elapsed,'policy':policy,'summary':final}
    results.append(row)
    (a.output_dir/'comparison.json').write_text(json.dumps(results,indent=2)+'\n')
    print(json.dumps({'run':label,'cpu_percent':row['cpu_percent_one_core'],'kernel_queue_ms':final['kernel_queue_ms'],'handoff_ms':final['handoff_batch_max_ms'],'late':final['playout']['late_packets'],'missing_frames':final['playout']['missing_frames'],'underruns':final['playout']['underruns']},ensure_ascii=False),flush=True)
