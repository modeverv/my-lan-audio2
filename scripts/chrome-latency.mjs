// Explicitly requested CDP/Web Audio experiment, using an isolated local Chrome profile.
import {spawn} from 'node:child_process';
import {mkdir,readFile,writeFile,open} from 'node:fs/promises';
import {createInterface} from 'node:readline';
import {resolve,join} from 'node:path';
import {performance} from 'node:perf_hooks';
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const root=resolve(import.meta.dirname,'..');
const run=join(root,'runs','chrome-cdp-'+Date.now());await mkdir(run,{recursive:true});
const clock=spawn(join(root,'target/render-tap/qpc-clock.exe'),[],{windowsHide:true});
const pendingClock=[];createInterface({input:clock.stdout}).on('line',l=>pendingClock.shift()?.(Number(l)));
function qpc(){return new Promise(r=>{pendingClock.push(r);clock.stdin.write('q\n');});}
function bounds(samples){const lower=Math.max(...samples.map(s=>s.lower)),upper=Math.min(...samples.map(s=>s.upper));if(lower>upper)throw Error('Clock calibration intervals inconsistent');return {lower,upper,mid:(lower+upper)/2,error_ms:(upper-lower)/2};}
async function clockCalibration(){const samples=[];for(let i=0;i<50;i++){const before=performance.now();const remote=await qpc();const after=performance.now();samples.push({lower:remote-after,upper:remote-before});}return {samples,...bounds(samples)};}
const profile=join(run,'profile');
const chrome=spawn('C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',[
    '--remote-debugging-address=127.0.0.1','--remote-debugging-port=9223','--user-data-dir='+profile,
    '--no-first-run','--no-default-browser-check','about:blank'],{windowsHide:true,stdio:'ignore'});
let ws,id=0;const requests=new Map();const captures=[];
function command(method,params={}){return new Promise((resolve,reject)=>{const n=++id;const timer=setTimeout(()=>{requests.delete(n);reject(Error('CDP timeout '+method));},15000);requests.set(n,{resolve,reject,timer});ws.send(JSON.stringify({id:n,method,params}));});}
async function js(expression){const r=await command('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true,userGesture:true,allowUnsafeEvalBlockedByCSP:false});if(r.exceptionDetails)throw Error(JSON.stringify(r.exceptionDetails));return r.result.value;}
async function browserCalibration(){const samples=[];for(let i=0;i<50;i++){const before=performance.now();const remote=await js('performance.now()');const after=performance.now();samples.push({lower:before-remote-0.1,upper:after-remote+0.1});}return {samples,...bounds(samples)};}
function stat(v){v.sort((a,b)=>a-b);return v.length?{count:v.length,min:v[0],p50:v[Math.ceil(v.length*.5)-1],p99:v[Math.ceil(v.length*.99)-1],max:v.at(-1)}:null;}
try{
    let tabs;for(let i=0;i<100;i++){try{tabs=await (await fetch('http://127.0.0.1:9223/json/list')).json();if(tabs.some(t=>t.url==='about:blank'))break;}catch{}await sleep(100);}
    const tab=tabs?.find(t=>t.url==='about:blank');if(!tab)throw Error('Isolated Chrome did not expose test page');
    ws=new WebSocket(tab.webSocketDebuggerUrl);await new Promise((r,j)=>{ws.onopen=r;ws.onerror=j;});
    ws.onmessage=e=>{const m=JSON.parse(e.data);const p=requests.get(m.id);if(p){requests.delete(m.id);clearTimeout(p.timer);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result);}};
    await command('Page.bringToFront');
    const version=await command('Browser.getVersion');
    const output=[];
    for(const hint of (process.env.CHROME_FINAL_MATRIX || process.env.CHROME_UDP_TO ? ['interactive'] : ['interactive','playback'])){
        console.log('Starting '+hint+' run: '+run);
        const setup=await js(`(async()=>{document.title='Chrome audio latency test';document.body.innerHTML='<h1>Chrome audio latency test</h1><p>Quiet test pulses; closes automatically.</p>';globalThis.audio=new AudioContext({latencyHint:${JSON.stringify(hint)}});await audio.resume();globalThis.signal=audio.createBuffer(1,Math.round(audio.sampleRate*.02),audio.sampleRate);const d=signal.getChannelData(0);for(let i=0;i<d.length;i++)d[i]=.005*Math.cos(2*Math.PI*1000*i/audio.sampleRate);return {state:audio.state,sampleRate:audio.sampleRate,baseLatency:audio.baseLatency,outputLatency:audio.outputLatency};})()`);
        if(setup.state!=='running')throw Error('AudioContext not running');
        const count=30,duration=32;
        const current=[];
        const backends=process.env.CHROME_UDP_TO ? ['process-include'] : process.env.CHROME_FINAL_MATRIX ? ['process-include','process-exclude','legacy','pre','post','min'] : ['process-include','legacy'];
        if(process.env.CHROME_KS_FILTER)backends.push('ks');
        for(const backend of backends){
            const prefix=join(run,hint+'-'+backend);const log=await open(prefix+'.log','wx');const summary=await open(prefix+'.summary.json','wx');
            const args=['--backend',['pre','post'].includes(backend)?'legacy':backend,'--seconds',String(duration),'--pulse-probe','--measure-level','--output',prefix+'.jsonl'];if(backend==='process-include')args.push('--pid',String(chrome.pid));
            if(['pre','post'].includes(backend))args.push('--tap',backend);
            if(backend==='ks')args.push('--ks-filter',process.env.CHROME_KS_FILTER,'--ks-pin','1','--ks-frames','48','--ks-position');
            if(process.env.CHROME_UDP_TO && backend==='process-include')args.push('--udp-to',process.env.CHROME_UDP_TO,'--udp-workers',process.env.CHROME_UDP_WORKERS||'1');
            const child=spawn(join(root,'target/release/sender-windows.exe'),args,{windowsHide:true,stdio:['ignore',summary.fd,log.fd]});
            const item={backend,child,prefix,log,summary,done:new Promise(r=>child.on('exit',code=>r(code)))};captures.push(item);current.push(item);
        }
        await sleep(700);
        const nativeBefore=await clockCalibration(),browserBefore=await browserCalibration();
        const pulses=[];
        for(let i=0;i<count;i++){
            // Nonuniform gaps make a shifted pulse sequence easier to reject. No CDP send time is used as onset.
            await sleep(610+(i*137%290));
            pulses.push(await js(`(()=>{const source=audio.createBufferSource();source.buffer=signal;source.connect(audio.destination);const contextTime=audio.currentTime;const before=performance.now();source.start();const after=performance.now();return {id:${i},before,after,contextTime,outputTimestamp:audio.getOutputTimestamp()};})()`));
            if((i+1)%10===0)console.log(hint+': '+(i+1)+' pulses');
        }
        const nativeAfter=await clockCalibration(),browserAfter=await browserCalibration();
        const native=bounds([...nativeBefore.samples,...nativeAfter.samples]);
        const browser=bounds([...browserBefore.samples,...browserAfter.samples]);
        const mapping={lower:native.lower+browser.lower,upper:native.upper+browser.upper};mapping.mid=(mapping.lower+mapping.upper)/2;mapping.error_ms=(mapping.upper-mapping.lower)/2;
        const results=[];
        for(const c of current){
            const exit=await c.done;await c.log.close();await c.summary.close();
            const records=(await readFile(c.prefix+'.jsonl','utf8')).trim().split('\n').map(l=>JSON.parse(l));
            const detections=records.filter(r=>r.event==='pulse');const used=new Set(),pairs=[];let unmatched=0;
            for(const d of detections){const receive=d.capture_time_100ns/10000;
                const source=pulses.findLast(p=>p.before+mapping.mid<=receive);
                if(!source||receive-source.before-mapping.mid>400||used.has(source.id)){unmatched++;continue;}
                // Include onset timestamp quantization separately from clock-map uncertainty.
                used.add(source.id);pairs.push({id:source.id,latency_ms:receive-source.before-mapping.mid,lower_ms:receive-source.after-mapping.upper-0.1,upper_ms:receive-source.before-mapping.lower+0.1});
            }
            results.push({backend:c.backend,exit,detected: detections.length,unmatched,missing:count-used.size,pairs,latency_ms:stat(pairs.map(p=>p.latency_ms)),startup:records.find(r=>r.event==='startup'),end:records.find(r=>r.event==='capture_end')});
        }
        output.push({hint,setup,mapping,nativeBefore,nativeAfter,browserBefore,browserAfter,pulses,results});
        await js('audio.close()');await writeFile(join(run,'results.json'),JSON.stringify({version,chrome_pid:chrome.pid,run,output},null,2));
        console.log(JSON.stringify({hint,clock_error_ms:mapping.error_ms,results:results.map(r=>({backend:r.backend,detected:r.detected,unmatched:r.unmatched,missing:r.missing,latency_ms:r.latency_ms}))}));
    }
    console.log('RESULT_DIR='+run);
}finally{
    for(const c of captures)if(c.child.exitCode===null)c.child.kill();
    if(ws?.readyState===WebSocket.OPEN){try{await command('Browser.close');}catch{}ws.close();}
    clock.stdin.end();
}
