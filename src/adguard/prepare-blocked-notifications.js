// Persist these immutable batches before delivery so retries reuse the same payload.
const known = new Set();
for (const {json: batch} of $('Read notification batches').all()) {
  for (const id of JSON.parse(batch.query_ids || '[]')) known.add(id);
}
const queries = $('Read blocked queries').all().map(i => i.json)
  .filter(q => q.query_id && q.blocked === true && !known.has(q.query_id));
const groups = new Map();
for (const q of queries) {
  if (known.has(q.query_id)) continue;
  known.add(q.query_id);
  const domain = q.domain.toLowerCase().replace(/\.$/, '');
  const key = JSON.stringify([domain, q.client]);
  if (!groups.has(key)) groups.set(key, {domain, client: q.client_name?.trim() || q.client, queries: []});
  groups.get(key).queries.push(q);
}
const rows = [...groups.values()].sort((a,b) => b.queries.length-a.queries.length || a.domain.localeCompare(b.domain) || a.client.localeCompare(b.client));
const escape = s => String(s).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const time = value => new Intl.DateTimeFormat('en-AU', {timeZone:'Australia/Brisbane', day:'2-digit',month:'short',year:'numeric',hour:'numeric',minute:'2-digit',timeZoneName:'short'}).format(new Date(value));
const batches = [];
// A conservative HTML-size limit leaves room for Matrix encryption overhead.
let chunk = [], size = 0;
function flush() {
  if (!chunk.length) return;
  const records = chunk.flatMap(r => r.queries);
  const times = records.map(q => Date.parse(q.queried_at));
  const message = '<h3>AdGuard blocked requests</h3><p>' + time(Math.min(...times)) + ' – ' + time(Math.max(...times)) + '</p><p><strong>' + records.length + ' blocked requests · ' + new Set(chunk.map(r=>r.domain)).size + ' domains · ' + new Set(records.map(q=>q.client)).size + ' clients</strong></p><table><thead><tr><th>Domain</th><th>Client</th><th>Blocked requests</th></tr></thead><tbody>' + chunk.map(r=>'<tr><td>'+escape(r.domain)+'</td><td>'+escape(r.client)+'</td><td>'+r.queries.length+'</td></tr>').join('') + '</tbody></table>';
  batches.push({json:{batch_id:'adguard-'+$execution.id+'-'+batches.length,message,query_ids:JSON.stringify(records.map(q=>q.query_id)),delivered:false,matrix_event_id:'',prepared_at:new Date().toISOString()}});
  chunk=[];size=0;
}
for (const row of rows) {
  const length = (escape(row.domain).length+escape(row.client).length)*3+100;
  if (size+length>12000 || chunk.length>=40) flush();
  chunk.push(row);size+=length;
}
flush();
return batches;
