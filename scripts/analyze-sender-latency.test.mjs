import {test} from 'node:test';
import assert from 'node:assert/strict';
import {parseLine,distribution,correlate,analyze} from './analyze-sender-latency.mjs';
test('64-bit wire IDs match without floating point rounding',()=>{
  const w=parseLine('{"event":"udp","wire_session":18446744073709551615,"sequence":4}');
  const m=parseLine('{"event":"receive","session_id":18446744073709551615,"sequence":4,"arrival_excess_ms":14}');
  assert.equal(w.wire_session,'18446744073709551615');
  assert.equal(correlate([w],[m]).matched,1);
  assert.equal(correlate([w],[{...m,session_id:'18446744073709551614'}]).matched,0);
});
test('64-bit sequence and sample keys preserve adjacent packets and reordering',()=>{
  const m=parseLine('{"event":"receive","session_id":7,"sequence":9007199254740993,"first_sample":18446744073709551615}');
  const before=parseLine('{"event":"receive","session_id":7,"sequence":9007199254740992,"first_sample":18446744073709551614}');
  const w={...m,event:'udp',wire_session:'7'};
  const result=correlate([w],[m,before,m]);
  assert.equal(result.matched,2);assert.equal(result.receive_reordered,1);assert.equal(result.receive_duplicates,1);
  assert.equal(result.sample_key_mismatches,0);
});
test('strict thresholds, nearest-rank percentiles, empty input',()=>{
  assert.equal(distribution([]).max,null);
  const s=distribution([...Array(998).fill(1000),5000,20000]);
  assert.equal(s.p99_9,5000);assert.equal(s.over_1ms,2);assert.equal(s.over_5ms,1);
});
test('startup exclusion does not alter full run or disguise failed sends',()=>{
  const rows=[{event:'startup'},{event:'capture_end',start_qpc_100ns:100},
    {event:'udp',wire_session:'7',sequence:'0',outcome:'deadline',acquired_100ns:101,dequeued_100ns:100101},
    {event:'udp',wire_session:'7',sequence:'1',outcome:'sent',acquired_100ns:60000100,published_100ns:60000110,
      send_start_100ns:60000120,send_end_100ns:60000130}];
  const s=analyze(rows);
  assert.equal(s.all.outcomes.deadline,1);assert.equal(s.after_first_5s.outcomes.deadline,undefined);
  assert.equal(s.all.expired_age_us.max,10000);assert.equal(s.all.send_call_us.count,1);
  assert.throws(()=>analyze([{event:'startup'}]),/Incomplete/);
});
