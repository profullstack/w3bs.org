const root = document.querySelector('#dns-names');
const status = document.querySelector('#dns-status');
let names = [];
const el = (tag,text,props={}) => Object.assign(document.createElement(tag),{...(text===null?{}:{textContent:text}),...props});
const api = async(path,init={}) => {
  const response=await fetch(path,{...init,headers:{accept:'application/json','content-type':'application/json'},cache:'no-store'});
  const body=await response.json();
  if(!response.ok)throw Error(body.error?.message||'The request failed. Please retry.');
  return body;
};
const help={AAAA:'IPv6 address, for example 2606:4700:4700::1111',CNAME:'Hostname only, for example box.example.com',MX:'Mail hostname, for example mx.example.com; lower priority wins',TXT:'Verification text or a key, up to 1024 bytes'};
function render(){
  root.replaceChildren();
  const query=document.querySelector('#dns-filter').value.trim().toLowerCase();
  const match=n=>query.endsWith('.*')?n.name.startsWith(query.slice(0,-1)):n.name.includes(query);
  const visible=names.filter(match);
  status.textContent=`${visible.length} name${visible.length===1?'':'s'} in your W3BS account.`;
  for(const item of visible){
    const card=el('article',null,{className:'net-dns-card'});
    card.append(el('h2',item.name),el('p',`${item.name}.w3bs.org`,{className:'net-help'}));
    if(item.status!=='active'||item.namespace_status!=='active'){
      card.append(el('p',`DNS editing waits until both the name and namespace are active. Name: ${item.status}; namespace: ${item.namespace_status}.`,{className:'net-status'}));root.append(card);continue;
    }
    const table=el('table',null,{className:'net-dns-table'}),head=el('tr',null);
    for(const title of ['Type','Value','TTL','Priority',''])head.append(el('th',title));
    const thead=el('thead',null);thead.append(head);table.append(thead);
    const tbody=el('tbody',null);table.append(tbody);
    const feedback=el('p','',{className:'net-status'});feedback.setAttribute('role','status');feedback.setAttribute('aria-live','polite');
    const perform=async(button,fn)=>{button.disabled=true;feedback.textContent='Saving…';feedback.classList.remove('net-error');try{const data=await fn();item.records=data.records;drawRows();feedback.textContent='Published to this W3BS instance. Resolver caches follow the record TTL.';}catch(error){feedback.textContent=error.message;feedback.classList.add('net-error');}finally{button.disabled=false;}};
    const path=`/api/net/v1/names/${encodeURIComponent(item.name)}/records`;
    const drawRows=()=>{
      tbody.replaceChildren();
      for(const record of item.records){
        const row=el('tr',null);for(const value of [record.type,record.value,`${record.ttl}s`,record.type==='MX'?record.priority:'—'])row.append(el('td',String(value)));
        const td=el('td',null),button=el('button','Remove',{type:'button',className:'button'});
        button.setAttribute('aria-label',`Remove ${record.type} ${record.value} from ${item.name}`);
        button.addEventListener('click',()=>perform(button,()=>api(`${path}/${encodeURIComponent(record.id)}`,{method:'DELETE'})));
        td.append(button);row.append(td);tbody.append(row);
      }
      if(!item.records.length){const row=el('tr',null),td=el('td','No custom records. The W3BS gateway serves this name by default.');td.colSpan=5;row.append(td);tbody.append(row);}
    };
    drawRows();
    const scroll=el('div',null,{className:'net-dns-scroll'});scroll.append(table);card.append(scroll);
    const form=el('form',null,{className:'net-dns-form'});
    const field=(label,input)=>{input.setAttribute('aria-label',label);const wrapper=el('label',label);wrapper.append(input);form.append(wrapper);return wrapper;};
    const type=el('select',null,{name:'type'});for(const t of Object.keys(help))type.append(el('option',t,{value:t}));field('Type',type);
    const value=el('input',null,{name:'value',required:true,maxLength:1024,placeholder:help.AAAA});field('Value',value);
    const ttl=el('input',null,{name:'ttl',type:'number',value:'300',min:'60',max:'86400',required:true});field('TTL (seconds)',ttl);
    const priority=el('input',null,{name:'priority',type:'number',value:'10',min:'0',max:'65535'});const priorityLabel=field('MX priority',priority);priorityLabel.hidden=true;
    const button=el('button','Publish record',{type:'submit',className:'button primary'});form.append(button);
    type.addEventListener('change',()=>{value.placeholder=help[type.value];priorityLabel.hidden=type.value!=='MX';});
    form.addEventListener('submit',async event=>{event.preventDefault();await perform(button,()=>api(path,{method:'POST',body:JSON.stringify({type:type.value,value:value.value,ttl:Number(ttl.value),priority:Number(priority.value)})}));});
    card.append(form,feedback);root.append(card);
  }
}
document.querySelector('#net-dns-filter').addEventListener('submit',event=>{event.preventDefault();render();});
try{names=(await api('/api/net/v1/dns/mine')).names;render();}catch(error){status.textContent=error.message;status.classList.add('net-error');}
