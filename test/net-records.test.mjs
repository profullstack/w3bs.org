import test from 'node:test';
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {openStore} from '../src/store.mjs';
import {startServer} from '../src/server.mjs';
import {normalizeNetRecord} from '../src/net-records.mjs';
for(const databaseUrl of [null,...(process.env.W3BS_TEST_DATABASE_URL?[process.env.W3BS_TEST_DATABASE_URL]:[])]){
 test(`instance DNS records enforce local ownership and publish immediately (${databaseUrl?'postgres':'sqlite'})`,async()=>{
  const store=await openStore({path:':memory:',databaseUrl,seed:false});
  const app=await startServer({store,port:0,publicOrigin:'http://localhost:3000'});
  const accounts=store.accounts('http://localhost:3000');
  const suffix=randomUUID().slice(0,8),tld=`zone-${suffix}`,name=`dumb.${tld}`;
  const signup=await accounts.startSignup({username:`dns-${suffix}`,email:`dns-${suffix}@example.com`});
  const {user}=await accounts.consumeLink(signup.link),token=await accounts.createSession(user.id);
  const base=`http://localhost:${app.server.address().port}`;
  const request=(path,method='GET',body,auth=true)=>fetch(base+path,{method,headers:{'content-type':'application/json',...(auth?{cookie:`w3bs_session=${token}`}:{})},...(body?{body:JSON.stringify(body)}:{})});
  try{
   await store.network.requestName({tld,label:'dumb',ownerId:user.id,requestNamespace:true});
   await assert.rejects(()=>store.network.dns.put(name,user.id,{type:'TXT',value:'pending'}),/must be active/);
   await store.network.moderate({kind:'namespace',subject:tld,action:'approve',actor:user.id});
   await store.network.moderate({kind:'name',subject:name,action:'approve',actor:user.id});
   const redirectPath=`/api/net/v1/names/${name}/redirect`;
   const destination='https://example.com/a-page?campaign=w3bs';
   assert.equal((await request(redirectPath,'PUT',{target:destination,status:301},false)).status,401);
   await assert.rejects(()=>store.network.dns.setRedirect(name,'someone-else',destination,301),/Only the W3BS owner/);
   for(const target of ['javascript:alert(1)','http://example.com','https://user:pass@example.com',`https://${name}.w3bs.org/path`])
    assert.equal((await request(redirectPath,'PUT',{target,status:301})).status,400);
   assert.equal((await request(redirectPath,'PUT',{target:destination,status:307})).status,400);
   assert.equal((await request(redirectPath,'PUT',{target:destination,status:301})).status,200);
   const gateway=()=>fetch(base+'/anything?ignored=yes',{headers:{host:`${name}.w3bs.org`},redirect:'manual'});
   let moved=await gateway();assert.equal(moved.status,301);assert.equal(moved.headers.get('location'),destination);
   assert.equal((await store.network.resolveName(name)).redirectStatus,301);
   assert.equal((await(await request('/api/net/v1/dns/mine')).json()).names.find(n=>n.name===name).redirect_status,301);
   assert.equal((await request(`/api/net/v1/names/${name}/records`,'POST',{type:'CNAME',value:'box.example.com'})).status,409);
   await request(redirectPath,'PUT',{target:destination,status:302});
   moved=await gateway();assert.equal(moved.status,302);assert.equal(moved.headers.get('location'),destination);
   await request(redirectPath,'DELETE');assert.equal((await gateway()).status,200);
   const events=await store.network.events({limit:100});
   assert.ok(events.some(e=>e.type==='name.activated'&&e.data.redirectStatus===301&&e.data.target===destination));
   const path=`/api/net/v1/names/${name}/records`;
   assert.equal((await request(path,'POST',{type:'TXT',value:'secret'},false)).status,401);
   await assert.rejects(()=>store.network.dns.put(name,'someone-else',{type:'TXT',value:'hijack'}),/Only the W3BS owner/);
   const made=await request(path,'POST',{type:'TXT',value:'é'.repeat(300),ttl:120});
   assert.equal(made.status,200,await made.clone().text());
   const records=(await made.json()).records;assert.equal(records.length,1);
   assert.equal((await (await request(`/api/net/v1/dns/records?name=${name}`)).json()).records[0].value,'é'.repeat(300));
   assert.equal((await request(path,'POST',{type:'CNAME',value:'target.example.com'})).status,409);
   await request(`${path}/${records[0].id}`,'DELETE');
   const concurrent=await Promise.allSettled([
    store.network.dns.put(name,user.id,{type:'CNAME',value:'target.example.com'}),
    store.network.dns.put(name,user.id,{type:'TXT',value:'conflict'}),
   ]);
   assert.equal(concurrent.filter(r=>r.status==='fulfilled').length,1);
   const cname=(await store.network.dns.list(name))[0];
   await request(`${path}/${cname.id}`,'DELETE');
   assert.equal((await request(path,'POST',{type:'MX',value:'mx.example.com',priority:10,ttl:60})).status,200);
   assert.equal((await request(path,'POST',{type:'AAAA',value:'2606:4700:4700::1111'})).status,200);
   assert.equal((await request(redirectPath,'PUT',{target:destination,status:301})).status,409);
   const mine=await(await request('/api/net/v1/dns/mine')).json();
   assert.equal(mine.names.find(n=>n.name===name).records.length,2);
   assert.equal((await request('/net/records')).status,200);
   assert.match(await(await request('/net/records','GET',undefined,false)).text(),/Sign in to publish/);
   await store.network.moderate({kind:'namespace',subject:tld,action:'suspend',actor:user.id});
   assert.deepEqual(await store.network.dns.list(name),[]);
   assert.equal((await request(redirectPath,'PUT',{target:destination,status:301})).status,409);
   assert.equal((await request(path,'POST',{type:'TXT',value:'suspended'})).status,409);
  }finally{await app.close();}
 });
}
test('record validation rejects private addresses, URLs, invalid TTLs and self aliases',()=>{
 for(const bad of [
  {type:'A',value:'127.0.0.1'}, {type:'AAAA',value:'::1'}, {type:'AAAA',value:'::ffff:127.0.0.1'},
  {type:'AAAA',value:'fd00::1'}, {type:'AAAA',value:'fe80::1%eth0'}, {type:'AAAA',value:'fe80::1'}, {type:'CNAME',value:'https://example.com/'},
  {type:'CNAME',value:'dumb.fuck.w3bs.org'}, {type:'MX',value:'mx.example.com',priority:-1},
  {type:'TXT',value:'x',ttl:1}, {type:'TXT',value:'x'.repeat(1025)},
 ])assert.throws(()=>normalizeNetRecord(bad,'dumb.fuck'));
 assert.deepEqual(normalizeNetRecord({type:'MX',value:'MX.Example.com.',priority:0},'dumb.fuck'),{type:'MX',value:'mx.example.com',ttl:300,priority:0});
});
