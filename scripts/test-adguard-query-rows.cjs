// Run with: node scripts/test-adguard-query-rows.cjs
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('src/adguard/prepare-query-rows.js', 'utf8');
const workflow = JSON.parse(fs.readFileSync('src/adguard/adguard-query-log-archive-workflow.json', 'utf8'));
assert.equal(workflow.nodes.find(n => n.name === 'Prepare new query rows').parameters.jsCode, source);
const query = {
  time: '2026-10-30T01:30:00.123456789Z', client: '10.0.0.25',
  question: { class: 'IN', name: 'example.com', type: 'AAAA' },
  reason: 'FilteredBlackList', elapsedMs: '0.45', cached: true,
};
const key = q => JSON.stringify([q.time, q.client, q.question.class, q.question.name, q.question.type, '']);
function run(pages, known = []) {
  return vm.runInNewContext(`(function () { ${source}\n})()`, {
    $: name => name === 'Collection window'
      ? { first: () => ({ json: { since: '2026-10-30T01:00:00.000Z' } }) }
      : { all: () => known.map(query_id => ({ json: { query_id } })) },
    $input: { all: () => pages.map(json => ({ json })) },
  });
}
const distinct = { ...query, time: '2026-10-30T01:30:00.123456790Z' };
const old = { ...query, time: '2026-10-30T00:59:59Z' };
const pages = [{ data: [query, distinct], oldest: distinct.time }, { data: [query, old], oldest: old.time }];
const rows = run(pages, [key(query)]);
assert.equal(rows.length, 1, 'Skip stored and repeated records, retain distinct nanosecond timestamps');
assert.equal(rows[0].json.query_id, key(distinct));
assert.equal(rows[0].json.blocked, true);
assert.equal(rows[0].json.elapsed_ms, 0.45);
assert.equal(JSON.parse(rows[0].json.raw_json).time, distinct.time);
assert.equal(run([{ data: [] }]).length, 0);
assert.throws(() => run([{ data: [{ ...query, time: 'bad' }] }]), /Malformed/);
assert.throws(() => run([{ error: 'bad response' }]), /Invalid/);
assert.throws(() => run(Array(200).fill({ data: [query], oldest: query.time })), /pagination limit/);
console.log('AdGuard row preparation checks passed');
