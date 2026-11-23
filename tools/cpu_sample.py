#!/usr/bin/env python3
"""Per-thread-group CPU sampler (plan d'architecture, lot 0.1). Read-only on /proc.

    tools/cpu_sample.py <windows> <seconds>

Samples the daemon (`orchestrator daemon`), the workers (`orchestrator
session <account>`), the game server (`node ... tsx`) and the docker relay,
every <seconds>, <windows> times. Per window it prints, in percent of one
core: the daemon per thread group (thread names with their trailing number
trimmed: `brain-tick-3` counts in `brain-tick`), context switches, disk
writes, the workers' totals and the game server's threads. The same groups
are logged by the daemon itself once a minute (`cpu-profile threads`,
lot 0.2); this tool is the outside view and also covers the workers.
Two windows an hour apart should give close figures (the lot 0.1 proof).
"""
import os,sys,time,collections,re,subprocess
HZ=os.sysconf('SC_CLK_TCK')
def pids(pattern):
    out=[]
    for p in os.listdir('/proc'):
        if not p.isdigit(): continue
        try:
            cmd=open(f'/proc/{p}/cmdline','rb').read().replace(b'\0',b' ').decode(errors='replace')
        except Exception: continue
        if re.search(pattern,cmd.strip()): out.append((int(p),cmd.strip()))
    return out
def stat_cpu(path):
    try:
        s=open(path).read()
    except Exception: return None
    r=s[s.rfind(')')+2:].split()
    return (int(r[11])+int(r[12]))/HZ, int(r[11])/HZ, int(r[12])/HZ
def threads(pid):
    d={}
    try:
        for t in os.listdir(f'/proc/{pid}/task'):
            try:
                name=open(f'/proc/{pid}/task/{t}/comm').read().strip()
            except Exception: continue
            c=stat_cpu(f'/proc/{pid}/task/{t}/stat')
            if c: d[int(t)]=(name,c)
    except Exception: pass
    return d
def ctx(pid):
    v=n=0
    try:
        for t in os.listdir(f'/proc/{pid}/task'):
            for line in open(f'/proc/{pid}/task/{t}/status'):
                if line.startswith('voluntary_ctxt'): v+=int(line.split()[1])
                elif line.startswith('nonvoluntary_ctxt'): n+=int(line.split()[1])
    except Exception: pass
    return v,n
def rss(pid):
    try:
        for line in open(f'/proc/{pid}/status'):
            if line.startswith('VmRSS'): return int(line.split()[1])
    except Exception: return 0
    return 0
def io(pid):
    d={}
    try:
        for line in open(f'/proc/{pid}/io'):
            k,v=line.split(':'); d[k]=int(v)
    except Exception: pass
    return d
def snap():
    s={}
    dm=pids(r'orchestrator daemon$')
    s['daemon']=dm[0][0] if dm else None
    s['dthreads']=threads(s['daemon']) if s['daemon'] else {}
    s['dctx']=ctx(s['daemon']) if s['daemon'] else (0,0)
    s['dio']=io(s['daemon']) if s['daemon'] else {}
    s['drss']=rss(s['daemon']) if s['daemon'] else 0
    w={}
    for pid,cmd in pids(r'orchestrator session '):
        c=stat_cpu(f'/proc/{pid}/stat')
        if c: w[pid]=(cmd.split()[-1],c,rss(pid),len(os.listdir(f'/proc/{pid}/task')),ctx(pid))
    s['workers']=w
    o={}
    for label,pat in (('node','node .*tsx'),('docker-proxy','docker-proxy .*43594'),('qemu-ga','qemu-ga')):
        for pid,cmd in pids(pat):
            c=stat_cpu(f'/proc/{pid}/stat')
            if c: o[(label,pid)]=(c,rss(pid))
    s['others']=o
    s['nthreads_node']={ (l,p): threads(p) for (l,p) in o if l=='node'}
    cpu=open('/proc/stat').readline().split()[1:]
    s['sys']=[int(x)/HZ for x in cpu]
    s['t']=time.time()
    return s
def report(a,b):
    dt=b['t']-a['t']
    print(f"=== window {dt:.0f}s ending {time.strftime('%H:%M:%S',time.localtime(b['t']))} daemon pid {b['daemon']} rss {b['drss']//1024} MB threads {len(b['dthreads'])}")
    sysd=[y-x for x,y in zip(a['sys'],b['sys'])]
    tot=sum(sysd); idle=sysd[3]+sysd[4]
    print(f"machine: busy {100*(tot-idle)/dt:.0f}% of one core equivalents (user {100*sysd[0]/dt:.0f} nice {100*sysd[1]/dt:.0f} sys {100*sysd[2]/dt:.0f} iowait {100*sysd[4]/dt:.0f} irq {100*(sysd[5]+sysd[6])/dt:.0f} steal {100*sysd[7]/dt:.0f})")
    by=collections.defaultdict(lambda:[0.0,0.0,0.0,0])
    for tid,(name,c) in b['dthreads'].items():
        prev=a['dthreads'].get(tid)
        base=prev[1] if prev else (0,0,0)
        g=re.sub(r'[-_]?\d+$','',name)
        by[g][0]+=c[0]-base[0]; by[g][1]+=c[1]-base[1]; by[g][2]+=c[2]-base[2]; by[g][3]+=1
    total=sum(v[0] for v in by.values())
    print(f"daemon total {100*total/dt:.1f}% of a core; ctx/s vol {(b['dctx'][0]-a['dctx'][0])/dt:.0f} nonvol {(b['dctx'][1]-a['dctx'][1])/dt:.0f}; io write {(b['dio'].get('write_bytes',0)-a['dio'].get('write_bytes',0))/dt/1024:.0f} KB/s read {(b['dio'].get('read_bytes',0)-a['dio'].get('read_bytes',0))/dt/1024:.0f} KB/s syscw {(b['dio'].get('syscw',0)-a['dio'].get('syscw',0))/dt:.0f}/s syscr {(b['dio'].get('syscr',0)-a['dio'].get('syscr',0))/dt:.0f}/s")
    for g,v in sorted(by.items(),key=lambda x:-x[1][0]):
        if v[0]/dt<0.0005: continue
        print(f"  {g:18s} x{v[3]:<3d} {100*v[0]/dt:6.2f}%  (user {100*v[1]/dt:5.2f} sys {100*v[2]/dt:5.2f})")
    ws=[]
    for pid,(acct,c,r,nt,cx) in b['workers'].items():
        if pid in a['workers']:
            pa=a['workers'][pid]
            ws.append((c[0]-pa[1][0],c[1]-pa[1][1],c[2]-pa[1][2],r,nt,acct,(cx[0]-pa[4][0]),(cx[1]-pa[4][1])))
    if ws:
        n=len(ws); tot=sum(x[0] for x in ws); u=sum(x[1] for x in ws); s_=sum(x[2] for x in ws)
        ws.sort()
        print(f"workers n={n} total {100*tot/dt:.1f}% (user {100*u/dt:.1f} sys {100*s_/dt:.1f}) avg {100*tot/dt/n:.2f}% median {100*ws[n//2][0]/dt:.2f}% max {100*ws[-1][0]/dt:.2f}% ({ws[-1][5]}) rss avg {sum(x[3] for x in ws)/n/1024:.1f} MB sum {sum(x[3] for x in ws)/1024:.0f} MB threads avg {sum(x[4] for x in ws)/n:.1f} ctx/s per worker vol {sum(x[6] for x in ws)/n/dt:.0f} nonvol {sum(x[7] for x in ws)/n/dt:.1f}")
    for key,(c,r) in b['others'].items():
        if key in a['others']:
            pa=a['others'][key][0]
            print(f"{key[0]} pid {key[1]}: {100*(c[0]-pa[0])/dt:.1f}% (user {100*(c[1]-pa[1])/dt:.1f} sys {100*(c[2]-pa[2])/dt:.1f}) rss {r//1024} MB")
    for key,th in b['nthreads_node'].items():
        pa=a['nthreads_node'].get(key,{})
        by2=collections.defaultdict(float)
        for tid,(name,c) in th.items():
            base=pa.get(tid,(None,(0,0,0)))[1]
            by2[re.sub(r'[-_#]?\d+$','',name)]+=c[0]-base[0]
        print('  node threads: '+', '.join(f"{g} {100*v/dt:.1f}%" for g,v in sorted(by2.items(),key=lambda x:-x[1])[:6]))
    sys.stdout.flush()
n=int(sys.argv[1]); w=float(sys.argv[2])
a=snap()
for i in range(n):
    time.sleep(w)
    b=snap(); report(a,b); a=b
