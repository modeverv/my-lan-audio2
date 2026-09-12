// Same-host UDP receive probe. Only test pulses are emitted by the isolated Chrome child.
import dgram from 'node:dgram';
import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
import {performance} from 'node:perf_hooks';
import {mkdir,writeFile,readFile} from 'node:fs/promises';
import {resolve,join} from 'node:path';
const root=resolve(import.meta.dirname,'..');
const golden=Buffer.from((await readFile(join(root,'docs/protocol-v1-golden.hex'),'utf8')).replace(/\s/g,''),'hex');
if(golden.length!==80||golden.readUInt16LE(6)!==72||golden.readUInt32LE(40)!==48000||golden.readUInt16LE(44)!==1||golden.readBigUInt64LE(64)!==8n||golden.readFloatLE(72)!==1||golden.readFloatLE(76)!==-.5)throw Error('Shared wire fixture failed independent decode');
const output=join(root,'runs','udp-local-'+Date.now());await mkdir(output,{recursive:true});
const clock=spawn(join(root,'target/render-tap/qpc-clock.exe'),[],{windowsHide:true});
const waiting=[];createInterface({input:clock.stdout}).on('line',l=>waiting.shift()?.(Number(l)));
async function calibrate(){const samples=[];for(let i=0;i<100;i++){
    const before=performance.now();const native=await new Promise(r=>{waiting.push(r);clock.stdin.write('q\n');});const after=performance.now();
    samples.push({lower:native-after,upper:native-before});
}return samples;}
const socket=dgram.createSocket('udp4');
const packets=[];let invalid=0,nonzero=0,peak=0;const errors=[];
socket.on('error',e=>errors.push(String(e)));
socket.on('message',b=>{
    const received=performance.now();
    if(b.length<72||b.toString('ascii',0,4)!=='LNAU'||b.readUInt16LE(4)!==1||b.readUInt16LE(6)!==72||b[15]!==1||b.length>1400){invalid++;return;}
    const channels=b[14],frames=b.readUInt16LE(44),flags=b.readUInt16LE(12);
    if(!channels||channels>8||!frames||b.length!==72+channels*frames*4||b.readUInt16LE(46)!==0||!b.readUInt32LE(40)||(flags&~31)!==0){invalid++;return;}
    let p=0;for(let i=72;i<b.length;i+=4){const v=b.readFloatLE(i);if(!Number.isFinite(v)){invalid++;return;}p=Math.max(p,Math.abs(v));}
    if((flags&1)&&p!==0){invalid++;return;}
    peak=Math.max(peak,p);if(p>0)nonzero++;
    packets.push({session:b.readBigUInt64LE(16).toString(),seq:Number(b.readBigUInt64LE(24)),sample:Number(b.readBigUInt64LE(32)),
        frames,channels,rate:b.readUInt32LE(40),flags,bytes:b.length,acquired:Number(b.readBigUInt64LE(48))/10000,
        published:Number(b.readBigUInt64LE(56))/10000,sent:Number(b.readBigUInt64LE(64))/10000,received});
});
function stats(v){v.sort((a,b)=>a-b);return v.length?{count:v.length,min:v[0],p50:v[Math.ceil(v.length*.5)-1],p99:v[Math.ceil(v.length*.99)-1],max:v.at(-1)}:null;}
let child;
try{
    await new Promise(r=>socket.bind(0,'127.0.0.1',r));
    const before=await calibrate();
    child=spawn(process.execPath,[join(root,'scripts/chrome-latency.mjs')],{cwd:root,windowsHide:true,
        env:{...process.env,CHROME_FINAL_MATRIX:'',CHROME_UDP_TO:'127.0.0.1:'+socket.address().port},stdio:['ignore','pipe','inherit']});
    let stdout='';child.stdout.on('data',b=>{stdout+=b;process.stdout.write(b);});
    const code=await new Promise((r,j)=>{child.on('exit',r);child.on('error',j);});
    // Child exits only after capture and nonblocking sender workers have finished.
    const after=await calibrate();
    const samples=[...before,...after],lower=Math.max(...samples.map(s=>s.lower)),upper=Math.min(...samples.map(s=>s.upper));
    if(lower>upper)throw Error('QPC calibration inconsistent');
    const mid=(lower+upper)/2;
    let duplicates=0,reordered=0,gaps=0,sampleGaps=0;const sessions=new Map();
    for(const p of packets){if(!sessions.has(p.session))sessions.set(p.session,[]);sessions.get(p.session).push(p);}
    for(const list of sessions.values()){
        let high=-1;for(const p of list){if(p.seq<high)reordered++;high=Math.max(high,p.seq);}
        const sorted=[...list].sort((a,b)=>a.seq-b.seq);let previous;
        for(const p of sorted){if(previous){if(p.seq===previous.seq){duplicates++;continue;}gaps+=p.seq-previous.seq-1;if(p.sample!==previous.sample+previous.frames)sampleGaps++;}previous=p;}
    }
    const sourceDir=stdout.match(/RESULT_DIR=(.+)/)?.[1]?.trim();
    const source=sourceDir?JSON.parse(await readFile(join(sourceDir,'results.json'),'utf8')):null;
    const sender=source?.output[0]?.results[0]?.end?.udp;
    const totalSent=sender?.workers.reduce((sum,w)=>sum+(w.sent||0),0);
    const result={code,sourceDir,workers_requested:process.env.CHROME_UDP_WORKERS||'1',packet_count:packets.length,
        sent_minus_received:totalSent===undefined?null:totalSent-packets.length,invalid,errors,nonzero,peak,sessions:sessions.size,
        duplicates,reordered,sequence_gaps:gaps,sample_gap_boundaries:sampleGaps,
        frame_counts:[...new Set(packets.map(p=>p.frames))],max_datagram_bytes:Math.max(0,...packets.map(p=>p.bytes)),
        qpc_mapping:{lower,upper,error_ms:(upper-lower)/2,samples},
        acquire_to_receive_ms:stats(packets.map(p=>p.received+mid-p.acquired)),
        send_to_receive_ms:stats(packets.map(p=>p.received+mid-p.sent)),sender,packets};
    await writeFile(join(output,'results.json'),JSON.stringify(result,null,2));
    const {packets:omit,qpc_mapping,...summary}=result;
    console.log(JSON.stringify({...summary,qpc_error_ms:qpc_mapping.error_ms,output},null,2));
    if(code!==0||invalid||errors.length||packets.length===0||nonzero===0||duplicates||gaps||sampleGaps||result.sent_minus_received!==0||sender?.pool_drops||sender?.workers.some(w=>w.deadline_drops||w.send_errors||w.would_block_drops||w.worker_panicked))process.exitCode=1;
}finally{socket.close();clock.stdin.end();if(child?.exitCode===null)child.kill();}
