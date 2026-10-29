#!/bin/sh
set -eu

# Real-agent upstream tests, only in a disposable --network none OCI container.
ROOT="$(CDPATH='' cd "$(dirname "$0")/../../.." && pwd)"
if [ "$(uname -s)" != Linux ] || [ "$(id -u)" != 0 ] ||
   { [ ! -f /.dockerenv ] && [ ! -f /run/.containerenv ]; }; then
	printf '%s\n' 'BLOCKED: requires a disposable root Linux OCI container' >&2; exit 1
fi
for command in ip nft python3 mount conntrack; do
	command -v "$command" >/dev/null || { printf 'missing command: %s\n' "$command" >&2; exit 1; }
done
[ -x "${AGENT_BIN:?mount the real agent read-only}" ]
[ -s "${FIXTURES:?mount rendered fixtures read-only}/dns-egress.nft" ]
ip -j address show | python3 -c 'import json,sys; a=json.load(sys.stdin); assert [x["ifname"] for x in a if "UP" in x["flags"]] == ["lo"] and all(x["ifname"]=="lo" or not x.get("addr_info") for x in a), "refusing existing networking"'
if [ ! -e /usr/libexec/gofro/dns-flows ]; then
	mkdir -p /usr/libexec/gofro
	ln -s "$ROOT/deploy/openwrt/root/usr/libexec/gofro/dns-flows" /usr/libexec/gofro/dns-flows
fi
cmp "$ROOT/deploy/openwrt/root/usr/libexec/gofro/dns-flows" /usr/libexec/gofro/dns-flows

tag="gde$$" owned=''
r="${tag}r" c="${tag}c" w="${tag}w" v="${tag}v"
cleanup() {
	for ns in $owned; do
		for pid in $(ip netns pids "$ns"); do kill "$pid" 2>/dev/null || true; done
		ip netns del "$ns"
	done
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
for ns in "$r" "$c" "$w" "$v"; do
	ip netns add "$ns"
	owned="$owned $ns"
	ip -n "$ns" link set lo up
done
ip -n "$r" link add lan0 type veth peer name client0 netns "$c"
ip -n "$r" link add wan0 type veth peer name peer0 netns "$w"
ip -n "$r" link add gt0 type veth peer name vpn0 netns "$v"
for pair in "$r lan0" "$c client0" "$r wan0" "$w peer0" "$r gt0" "$v vpn0"; do
	ns=${pair% *} device=${pair#* }
	ip -n "$ns" link set "$device" up
done
ip -n "$r" addr add 192.168.0.1/24 dev lan0
ip -n "$c" addr add 192.168.0.2/24 dev client0
ip -n "$r" -6 addr add 2001:db8:1::1/64 dev lan0 nodad
ip -n "$c" -6 addr add 2001:db8:1::2/64 dev client0 nodad
ip -n "$r" addr add 192.0.2.1/24 dev wan0
ip -n "$w" addr add 192.0.2.2/24 dev peer0
ip -n "$r" addr add 10.202.0.2/24 dev gt0
ip -n "$v" addr add 10.202.0.1/24 dev vpn0
ip -n "$v" addr add 1.1.1.1/32 dev lo
ip -n "$v" addr add 1.0.0.1/32 dev lo
ip -n "$w" addr add 1.1.1.1/32 dev lo
ip -n "$w" addr add 1.0.0.1/32 dev lo
ip -n "$c" route add default via 192.168.0.1
ip -n "$v" route add default via 10.202.0.2
ip -n "$w" route add default via 192.0.2.1
ip -n "$r" route add default via 192.0.2.2
ip -n "$r" rule add pref 80 fwmark 0x10000/0x30000 lookup main
ip -n "$r" rule add pref 81 fwmark 0x20000/0x30000 lookup 100
ip -n "$r" route add default dev gt0 table 100 metric 10 proto 186
ip -n "$r" route add unreachable default table 100 metric 32767 proto 186
ip -n "$r" route add 192.168.0.0/24 dev lan0 table 100 proto 186
# shellcheck disable=SC2016 # device is expanded only inside the owned namespace.
ip netns exec "$r" sh -eu -c '
 mount -t proc -o nosuid,nodev,noexec proc /proc
 printf "1\n" > /proc/sys/net/ipv4/ip_forward
 for device in all default lan0 wan0 gt0; do printf "0\n" > "/proc/sys/net/ipv4/conf/$device/rp_filter"; done
'

python3 - "$r" "$c" "$w" "$v" "$AGENT_BIN" "$FIXTURES" <<'PY'
import hashlib
import json
import pathlib
import select
import subprocess
import sys
import tempfile
import time

r, c, w, v, binary, fixtures = sys.argv[1:]
def run(ns, *args, **kwargs):
    return subprocess.run(['ip', 'netns', 'exec', ns, *args], check=True, text=True, timeout=20, **kwargs)

server_code = r'''
import json,pathlib,socket,struct,sys,threading
address,log=sys.argv[1:]
lock=threading.Lock()
def name(data,off):
 out=[]
 while data[off]:
  n=data[off];off+=1;out.append(data[off:off+n].decode());off+=n
 return '.'.join(out),off+1
def answer(data,peer,transport):
 ident,flags,qd,an,ns,ar=struct.unpack_from('!6H',data)
 host,off=name(data,12);question_type=struct.unpack_from('!H',data,off)[0];end=off+4;cursor=end;options=[];do=False
 for _ in range(ar):
  _,cursor=name(data,cursor)
  kind,klass,ttl,length=struct.unpack_from('!HHIH',data,cursor);cursor+=10
  if kind==41:
   do=bool(ttl&0x8000);p=cursor
   while p<cursor+length:
    code,size=struct.unpack_from('!HH',data,p);options.append(code);p+=4+size
  cursor+=length
 event={'name':host,'peer':peer[0],'transport':transport,'options':options,'do':do,'qtype':question_type}
 with lock:
  with open(log,'a') as f:f.write(json.dumps(event)+'\n')
 question=data[12:end]
 if host.startswith('tc.') and transport=='udp':
  return struct.pack('!6H',ident,0x8380,1,0,0,0)+question
 if question_type==28:
  records=b'\xc0\x0c'+struct.pack('!HHIH',28,1,60,16)+socket.inet_pton(socket.AF_INET6,'2001:db8:2::80')
 else:
  records=b'\xc0\x0c'+struct.pack('!HHIH',1,1,60,4)+socket.inet_aton('203.0.113.80')
 count=1
 if host.startswith('large.'):
  for _ in range(24):
   payload=b'\xf0'+b'x'*240
   records+=b'\xc0\x0c'+struct.pack('!HHIH',16,1,60,len(payload))+payload;count+=1
 return struct.pack('!6H',ident,0x8180,1,count,0,0)+question+records
udp=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);udp.bind((address,53))
tcp=socket.socket();tcp.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1);tcp.bind((address,53));tcp.listen(16)
def udp_loop():
 while True:
  data,peer=udp.recvfrom(65535);udp.sendto(answer(data,peer,'udp'),peer)
def read(stream,size):
 data=b''
 while len(data)<size:
  part=stream.recv(size-len(data))
  if not part:raise EOFError()
  data+=part
 return data
def tcp_loop():
 while True:
  stream,peer=tcp.accept()
  with stream:
   stream.settimeout(5)
   data=read(stream,struct.unpack('!H',read(stream,2))[0]);response=answer(data,peer,'tcp')
   stream.sendall(struct.pack('!H',len(response))+response)
threading.Thread(target=udp_loop,daemon=True).start()
threading.Thread(target=tcp_loop,daemon=True).start()
print('ready',flush=True);threading.Event().wait()
'''

client_code = r'''
import json,socket,struct,sys
host,transport,ecs,server,qtype=sys.argv[1:]
family=socket.AF_INET6 if ':' in server else socket.AF_INET
qname=b''.join(bytes([len(s)])+s.encode() for s in host.split('.'))+b'\0'
question=qname+struct.pack('!HH',int(qtype),1)
options=struct.pack('!HH',65001,1)+b'x'
if ecs=='yes':options+=struct.pack('!HHHBB',8,7,1,24,0)+bytes([203,0,113])
packet=struct.pack('!6H',29001,0x100,1,0,0,1)+question+b'\0'+struct.pack('!HHIH',41,1232,0x8000,len(options))+options
with socket.socket(family,socket.SOCK_DGRAM if transport=='udp' else socket.SOCK_STREAM) as s:
 s.settimeout(7);s.connect((server,53))
 if transport=='udp':s.send(packet);data=s.recv(65535)
 else:
  s.sendall(struct.pack('!H',len(packet))+packet)
  with s.makefile('rb') as f:
   length=struct.unpack('!H',f.read(2))[0];data=f.read(length)
 ident,flags,qd,an,ns,ar=struct.unpack_from('!6H',data)
 assert ident==29001 and qd==1 and data[12:12+len(question)]==question
 print(json.dumps({'rcode':flags&15,'answer_count':an}))
'''

with tempfile.TemporaryDirectory(prefix='gofro-dns-egress-',dir='/run') as temporary:
    root=pathlib.Path(temporary)
    processes=[]
    def events(kind):
        path=root/(kind+'.jsonl')
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
    def fixture(name='dns-egress'):
        run(r,'nft','-f',str(pathlib.Path(fixtures)/(name+'.nft')))
    def query(host,transport='udp',expected=0,ecs='yes',server='192.168.0.1',qtype=1):
        result=json.loads(run(c,'python3','-c',client_code,host,transport,ecs,server,str(qtype),capture_output=True).stdout)
        assert result['rcode']==expected,(host,result)
        return result
    try:
        for kind,ns,address in [('direct',r,'127.0.0.1'),('vpn',v,'1.1.1.1'),('warp',v,'1.0.0.1'),('wan',w,'1.1.1.1'),('wan-warp',w,'1.0.0.1')]:
            process=subprocess.Popen(['ip','netns','exec',ns,'python3','-u','-c',server_code,address,str(root/(kind+'.jsonl'))],stdout=subprocess.PIPE,text=True)
            processes.append(process)
            assert select.select([process.stdout],[],[],5)[0] and process.stdout.readline().strip()=='ready'
        key='A'*43+'='
        routing={'domain_rules':[{'name':target,'enabled':True,'matcher':{'type':'suffix','value':target+'.example'},'target':target} for target in ('direct','vpn','warp','block')],
                 'ip_rules':[],'default_target':'vpn','mode':'rules'}
        (root/'config.json').write_text(json.dumps({'vpn_enabled':True,'active_server_key':key,'servers':[{'name':'Fixture','endpoint':'192.0.2.254:51820','public_key':key}], 'routing':routing}))
        (root/'empty.dat').write_bytes(b'')
        salt=bytes(range(32));digest=hashlib.scrypt(b'namespace-only',salt=salt,n=16384,r=8,p=1,dklen=32)
        (root/'password').touch(mode=0o600)
        (root/'password').write_text(f'gofro-scrypt-v1$16384$8$1${salt.hex()}${digest.hex()}')
        args=[binary,'--lan-interface','lan0','--lan-subnet','192.168.0.0/24','--listen','127.0.0.1:8080','--http-listen','192.168.0.1:8081','--https-listen','192.168.0.1:8443','--dns-listen','192.168.0.1:5353','--dns-upstream','127.0.0.1:53','--mode-command','/bin/false']
        for flag,name in [('config','config.json'),('geosite','empty.dat'),('geoip','empty.dat'),('tls-cert','cert.pem'),('tls-key','key.pem'),('admin-password','password'),('setup-code','setup-code'),('management-dir','management'),('routing-state','routing.sqlite')]:args+=['--'+flag,str(root/name)]
        log=(root/'agent.log').open('w')
        agent=subprocess.Popen(['ip','netns','exec',r,*args],stdout=log,stderr=subprocess.STDOUT)
        processes.append(agent)
        for _ in range(150):
            assert agent.poll() is None,(root/'agent.log').read_text()
            if 'Gofro agent started' in (root/'agent.log').read_text():break
            time.sleep(.1)
        else:raise AssertionError('real agent did not become ready')
        fixture()
        for target,source,kind in [('direct','127.0.0.1','direct'),('vpn','10.202.0.2','vpn'),('warp','10.202.1.2','warp')]:
            for transport in ('udp','tcp'):
                host=transport+'.'+target+'.example'
                query(host,transport)
                received=[row for row in events(kind) if row['name']==host]
                assert received and all(row['peer']==source and row['do'] and 65001 in row['options'] for row in received),received
                assert all((8 in row['options'])==(target=='direct') for row in received),received
            print('PASS: actual',target,'DNS source, UDP/TCP ingress, EDNS and no shared resolver cache',flush=True)
        query('unmatched.example')
        assert any(row['name']=='unmatched.example' and row['peer']=='10.202.0.2' for row in events('vpn'))
        for target,source in [('vpn','10.202.0.2'),('warp','10.202.1.2')]:
            for transport in ('udp','tcp'):
                host=transport+'6.'+target+'.example'
                assert query(host,transport,server='2001:db8:1::1',qtype=28)['answer_count']==0
                received=[row for row in events(target) if row['name']==host]
                assert received and all(row['peer']==source and row['qtype']==28 and 8 not in row['options'] for row in received),received
        print('PASS: IPv6 client DNS and AAAA questions use protected IPv4 upstreams without an IPv6 escape',flush=True)
        query('printer.home.arpa')
        assert any(row['name']=='printer.home.arpa' for row in events('direct'))
        for target,source in [('vpn','10.202.0.2'),('warp','10.202.1.2')]:
            for prefix in ('tc','large'):
                host=prefix+'.'+target+'.example'
                query(host)
                received=[row for row in events(target) if row['name']==host]
                assert {row['transport'] for row in received}=={'udp','tcp'},received
                assert all(row['peer']==source and 8 not in row['options'] for row in received),received
        print('PASS: truncated and full-buffer UDP fall back to TCP on the same VPN/WARP path',flush=True)
        # Independent of the two resolver IPs, stale reserved-mark NAT tuples
        # must fail closed if another local socket reuses a DNS five-tuple.
        stale = r'''
import socket,struct,subprocess
qname=b'\x05stale\x07example\0'
packet=struct.pack('!6H',29002,0x100,1,0,0,0)+qname+struct.pack('!HH',1,1)
for port,first,second in ((42053,0x20000,0x60000),(42054,0x60000,0x20000)):
 with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as s:
  s.settimeout(1);s.setsockopt(socket.SOL_SOCKET,socket.SO_MARK,first)
  s.bind(('10.202.0.2',port));s.connect(('1.0.0.1',53));s.send(packet)
  assert s.recv(4096)[:2]==packet[:2]
  s.setsockopt(socket.SOL_SOCKET,socket.SO_MARK,second)
  try:s.send(packet);s.recv(4096)
  except (TimeoutError,PermissionError):pass
  else:raise AssertionError('stale DNS NAT binding escaped through the wrong path')
  subprocess.run(['conntrack','-D','-f','ipv4','-p','udp','-s','10.202.0.2','-d','1.0.0.1','--sport',str(port),'--dport','53'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  s.send(packet);assert s.recv(4096)[:2]==packet[:2]
'''
        run(r,'python3','-c',stale)
        print('PASS: reused local DNS sockets cannot retain non-WARP/WARP NAT bindings across mark changes',flush=True)
        before=len(events('direct'))+len(events('vpn'))+len(events('warp'))
        query('blocked.block.example',expected=5)
        assert len(events('direct'))+len(events('vpn'))+len(events('warp'))==before
        fixture('dns-egress-no-alias')
        before=len(events('warp'))
        query('no-alias.warp.example',expected=2)
        assert len(events('warp'))==before,'missing WARP alias leaked into ordinary VPN DNS'
        query('still-working.vpn.example')
        fixture()
        run(r,'ip','rule','del','pref','81')
        for target in ('vpn','warp'):
            query('no-rule.'+target+'.example',expected=2)
        assert events('wan')==[] and events('wan-warp')==[],'protected DNS escaped over WAN without the policy rule'
        run(r,'ip','rule','add','pref','81','fwmark','0x20000/0x30000','lookup','100')
        run(r,'ip','route','del','default','dev','gt0','table','100','metric','10')
        for target in ('vpn','warp'):
            query('no-tunnel.'+target+'.example',expected=2)
        query('still-working.direct.example')
        assert events('wan')==[] and events('wan-warp')==[]
        assert not any(row['name'].startswith(('no-alias.','no-rule.','no-tunnel.')) for row in events('direct')),'protected failure retried the local resolver'
        print('PASS: Block, missing WARP alias, missing policy rule and failed tunnel never leak or fall back to ISP DNS',flush=True)
        # Intentional VPN-off is not a transport failure: ordinary VPN DNS goes
        # Direct, while WARP remains locally refused without contacting either.
        run(r,'ip','route','add','default','dev','gt0','table','100','metric','10','proto','186')
        agent.terminate();agent.wait(timeout=5);log.close()
        saved=json.loads((root/'config.json').read_text());saved['vpn_enabled']=False
        (root/'config.json').write_text(json.dumps(saved))
        log=(root/'agent-off.log').open('w')
        agent=subprocess.Popen(['ip','netns','exec',r,*args],stdout=log,stderr=subprocess.STDOUT)
        processes.append(agent)
        for _ in range(150):
            assert agent.poll() is None,(root/'agent-off.log').read_text()
            if 'Gofro agent started' in (root/'agent-off.log').read_text():break
            time.sleep(.1)
        else:raise AssertionError('VPN-off agent did not become ready')
        fixture('dns-egress-off')
        before=len(events('vpn'))+len(events('warp'))
        query('disabled.warp.example',expected=5)
        query('disabled.vpn.example')
        assert any(row['name']=='disabled.vpn.example' for row in events('direct'))
        assert len(events('vpn'))+len(events('warp'))==before
        print('PASS: intentional VPN-off preserves Direct and refuses WARP locally',flush=True)
        print('PASS: actual DNS egress namespace regression complete; no WAN DNS queries',flush=True)
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:process.wait(timeout=5)
                except subprocess.TimeoutExpired:process.kill();process.wait(timeout=5)
        if 'log' in locals():
            log.close()
            if sys.exc_info()[0] is not None:print((root/'agent.log').read_text(),file=sys.stderr)
PY
