// Metadata only. Preserve 64-bit IDs as strings; compare sequences with BigInt.
import {readFileSync, writeFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
import {pathToFileURL} from 'node:url';

export function parseLine(line) {
  return JSON.parse(line.replace(/("(?:wire_session|session_id|sequence|first_sample)"\s*:\s*)(\d+)/g, '$1"$2"'));
}
function read(path) { return readFileSync(path,'utf8').trim().split(/\r?\n/).filter(Boolean).map(parseLine); }
export function distribution(values) {
  const v=values.filter(Number.isFinite).sort((a,b)=>a-b);
  const p=f=>v.length?v[Math.ceil(v.length*f)-1]:null;
  return {count:v.length,p50:p(.5),p99:p(.99),p99_9:p(.999),max:v.at(-1)??null,
    over_1ms:v.filter(x=>x>1000).length,over_5ms:v.filter(x=>x>5000).length};
}
const delta=(a,b)=>a!=null&&b!=null?(a-b)/10:NaN;
const intervals=(rows,key)=>rows.slice(1).map((r,i)=>delta(r[key],rows[i][key]));
const counts=(rows,key)=>Object.fromEntries([...new Set(rows.map(r=>r[key]))].map(k=>[k,rows.filter(r=>r[key]===k).length]));
function section(rows) {
  const c=rows.filter(r=>r.event==='capture'), w=rows.filter(r=>r.event==='wake'), u=rows.filter(r=>r.event==='udp');
  const sent=u.filter(r=>r.outcome==='sent');
  const first=c.filter((r,i)=>i===0||r.wake_time_100ns!==c[i-1].wake_time_100ns);
  return {
    capture_packets:c.length,captured_frames:c.reduce((n,r)=>n+r.frames,0),
    nonzero_packets:c.filter(r=>r.peak>1e-6).length,zero_peak_packets:c.filter(r=>r.peak===0).length,
    discontinuities:c.filter(r=>r.flags&1).length,timestamp_errors:c.filter(r=>r.flags&4).length,
    valid_device_positions:c.filter(r=>r.device_position_valid).length,
    frames_per_capture:counts(c,'frames'),packets_per_wake:counts(w,'packets'),frames_per_wake:counts(w,'frames'),
    wake_interval_us:distribution(intervals(w,'time_100ns')),
    active_wake_interval_us:distribution(intervals(w.filter(r=>r.packets>0),'time_100ns')),
    capture_interval_us:distribution(intervals(c,'capture_time_100ns')),
    get_buffer_us:distribution(c.map(r=>r.get_buffer_us)),drain_us:distribution(w.map(r=>r.drain_us)),
    wake_to_first_get_buffer_us:distribution(first.map(r=>delta(r.get_buffer_start_100ns,r.wake_time_100ns))),
    outcomes:counts(u,'outcome'),
    acquire_to_send_us:distribution(sent.map(r=>delta(r.send_start_100ns,r.acquired_100ns))),
    publish_to_send_us:distribution(sent.map(r=>delta(r.send_start_100ns,r.published_100ns))),
    acquire_to_publish_us:distribution(u.map(r=>delta(r.published_100ns,r.acquired_100ns))),
    publish_to_dequeue_us:distribution(u.map(r=>delta(r.dequeued_100ns,r.published_100ns))),
    send_call_us:distribution(u.map(r=>delta(r.send_end_100ns,r.send_start_100ns))),
    expired_age_us:distribution(u.filter(r=>r.outcome==='deadline').map(r=>delta(r.dequeued_100ns,r.acquired_100ns))),
  };
}
export function analyze(rows) {
  const startup=rows.find(r=>r.event==='startup'),end=rows.find(r=>r.event==='capture_end');
  if(!startup||!end)throw Error('Incomplete run: missing startup/capture_end');
  const cutoff=end.start_qpc_100ns+5*1e7;
  const time=r=>r.acquired_100ns??r.capture_time_100ns??r.time_100ns;
  const udp=rows.filter(r=>r.event==='udp');
  const worst=[...udp].filter(r=>r.send_end_100ns!=null)
    .sort((a,b)=>(b.send_end_100ns-b.send_start_100ns)-(a.send_end_100ns-a.send_start_100ns)).slice(0,5)
    .map(r=>({packet:r,neighbors:udp.filter(n=>n.wire_session===r.wire_session&&
      BigInt(n.sequence)>=BigInt(r.sequence)-2n&&BigInt(n.sequence)<=BigInt(r.sequence)+2n)}));
  const wakes=rows.filter(r=>r.event==='wake').sort((a,b)=>(b.interval_us??0)-(a.interval_us??0)).slice(0,5);
  return {startup,end,summary:rows.find(r=>r.event==='summary'),all:section(rows),
    after_first_5s:section(rows.filter(r=>time(r)>=cutoff)),worst_send_calls:worst,worst_wakes:wakes};
}
export function correlate(windows,mac) {
  const byKey=new Map(windows.filter(r=>r.event==='udp').map(r=>[`${r.wire_session}/${r.sequence}`,r]));
  const matches=mac.filter(r=>r.event==='receive').map(r=>({mac:r,windows:byKey.get(`${r.session_id}/${r.sequence}`)}));
  const seen=new Set(),last=new Map();let duplicates=0,reordered=0;
  for(const {mac:r} of matches){
    const key=`${r.session_id}/${r.sequence}`;
    if(seen.has(key))duplicates++;
    else if(last.has(r.session_id)&&BigInt(r.sequence)<last.get(r.session_id))reordered++;
    seen.add(key);
    if(!last.has(r.session_id)||BigInt(r.sequence)>last.get(r.session_id))last.set(r.session_id,BigInt(r.sequence));
  }
  return {received:matches.length,matched:matches.filter(r=>r.windows).length,
    sample_key_mismatches:matches.filter(r=>r.windows&&r.windows.first_sample!==r.mac.first_sample).length,
    receive_duplicates:duplicates,receive_reordered:reordered,
    note:'Join by wire session/sequence. Arrival excess is an interval difference, not one-way delay. Unmatched sends can lie outside receiver recording; do not count them as loss.',
    worst_arrival_excess:matches.filter(r=>r.windows).sort((a,b)=>b.mac.arrival_excess_ms-a.mac.arrival_excess_ms).slice(0,20)};
}
if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href){
  const [file,output,mac]=process.argv.slice(2);
  if(!file||!output)throw Error('Usage: node scripts/analyze-sender-latency.mjs WINDOWS.jsonl OUTPUT.json [MAC.jsonl]');
  const rows=read(file),result=analyze(rows);
  result.source={path:file,sha256:createHash('sha256').update(readFileSync(file)).digest('hex')};
  if(mac)result.mac_correlation=correlate(rows,read(mac));
  writeFileSync(output,JSON.stringify(result,null,2)+'\n');
  console.log(JSON.stringify({file,all:result.all,after_first_5s:result.after_first_5s,udp:result.end.udp},null,2));
}
