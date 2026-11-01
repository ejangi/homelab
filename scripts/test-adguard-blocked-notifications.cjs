const {strict: assert} = require('node:assert');
const {readFileSync} = require('node:fs');
const vm = require('node:vm');
const source = readFileSync('src/adguard/prepare-blocked-notifications.js','utf8');
const q = (id, client, name, domain='ADS.example.com.') => ({query_id:id,client,client_name:name,domain,blocked:true,queried_at:'2026-10-30T02:00:00Z'});
function run(queries,batches=[]) {
 return vm.runInNewContext('(function(){'+source+'})()', {$execution:{id:'test'},$:name=>({all:()=> (name==='Read blocked queries'?queries:batches).map(json=>({json}))})});
}
const queries=[q('a','1','Laptop'),q('b','1','Laptop'),q('c','2','<Phone>'),q('d','3','')];
let out=run(queries);
assert.equal(out.length,1);
assert.match(out[0].json.message, /<td>ads.example.com<\/td><td>Laptop<\/td><td>2<\/td>/);
assert.match(out[0].json.message,/&lt;Phone&gt;/);
assert.match(out[0].json.message,/<td>3<\/td><td>1<\/td>/);
assert.equal(JSON.parse(out[0].json.query_ids).length,4);
assert.equal(run(queries,out.map(i=>i.json)).length,0); // pending and delivered batches both reserve IDs
assert.equal(run([]).length,0);
assert.equal(run([q('new','1','Laptop')],out.map(i=>i.json)).length,0); // an outage cannot accumulate more reports
assert.equal(run([q('new','1','Laptop')],out.map(i=>({...i.json,delivered:true}))).length,1);
assert.equal(run([queries[0],queries[0]])[0].json.message.includes('1 blocked requests'),true);
assert.equal(run(Array.from({length:81},(_,i)=>q(String(i),'1','Laptop',i+'.example.com'))).length,2);
assert.equal(run(Array.from({length:41},(_,i)=>q(String(i),'1','Laptop',i+'.example.com'))).length,1);
console.log('AdGuard notification grouping, escaping, duplicate suppression, empty runs and splitting passed');
