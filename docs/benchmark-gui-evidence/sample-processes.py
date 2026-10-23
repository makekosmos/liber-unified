import subprocess,time,json,pathlib
out=pathlib.Path('/Users/user/Documents/projects/mundus-rs/liber-gpui/docs/benchmark-gui-evidence/isolated-process-samples.jsonl')
with out.open('w') as f:
 while True:
  start=time.time(); rows=[]
  for line in subprocess.check_output(['ps','-axo','pid,ppid,rss,pcpu,comm'],text=True).splitlines()[1:]:
   p=line.split(None,4)
   if len(p)==5:
    try: rows.append({'pid':int(p[0]),'ppid':int(p[1]),'rss_kib':int(p[2]),'cpu_pct':float(p[3]),'comm':p[4]})
    except ValueError: pass
  keep={r['pid'] for r in rows if '/tmp/liber-gui-benchmark/' in r['comm'] and '.app/' in r['comm']}
  while True:
   nxt=keep|{r['pid'] for r in rows if r['ppid'] in keep}
   if nxt==keep: break
   keep=nxt
  f.write(json.dumps({'time':start,'processes':[r for r in rows if r['pid'] in keep]})+'\n');f.flush()
  time.sleep(max(0,0.5-(time.time()-start)))
