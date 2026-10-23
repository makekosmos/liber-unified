import subprocess,time,json,pathlib
out=pathlib.Path('/Users/user/Documents/projects/mundus-rs/liber-gpui/docs/benchmark-gui-evidence/idle.json')
def snap():
 rows=[]
 for line in subprocess.check_output(['ps','-axo','pid,ppid,rss,time,comm'],text=True).splitlines()[1:]:
  p=line.split(None,4)
  if len(p)!=5:continue
  try:
   parts=p[3].split(':');cpu=sum(float(v)*60**i for i,v in enumerate(reversed(parts)))
   rows.append(dict(pid=int(p[0]),ppid=int(p[1]),rss=int(p[2]),cpu_seconds=cpu,comm=p[4]))
  except ValueError:continue
 result={}
 for name,root in [('Liber',91092),('Zotero',91172)]:
  ids={root}
  while True:
   nxt=ids|{r['pid'] for r in rows if r['ppid'] in ids}
   if nxt==ids:break
   ids=nxt
  selected=[r for r in rows if r['pid'] in ids];result[name]=dict(rss_mib=sum(r['rss'] for r in selected)/1024,cpu_seconds=sum(r['cpu_seconds'] for r in selected),pids=sorted(ids))
 return {'time':time.time(),'apps':result}
results=[]
for run in range(3):
 a=snap();time.sleep(30);b=snap();result={'run':run+1,'start':a,'end':b,'cpu_pct':{n:100*(b['apps'][n]['cpu_seconds']-a['apps'][n]['cpu_seconds'])/(b['time']-a['time']) for n in a['apps']}}
 results.append(result);out.write_text(json.dumps(results,indent=2));print(json.dumps(result),flush=True)
