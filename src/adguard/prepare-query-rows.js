// Keep the original nanosecond timestamp in query_id and raw_json. Date columns
// have millisecond precision, which cannot safely identify individual queries.
const window = $('Collection window').first().json;
const pages = $input.all().map(item => item.json);
const last = pages.at(-1);
if (!last || !Array.isArray(last.data)) throw new Error('Invalid AdGuard query-log response');
if (pages.length >= 200 && last.data.length && last.oldest && Date.parse(last.oldest) > Date.parse(window.since)) {
  throw new Error('AdGuard pagination limit reached before the collection window was covered');
}
const known = new Set($('Read recent stored queries').all().map(item => item.json.query_id).filter(Boolean));
const rows = [];
for (const page of pages) {
  if (!Array.isArray(page.data)) throw new Error('Invalid AdGuard query-log page');
  for (const query of page.data) {
    const time = Date.parse(query.time);
    if (!Number.isFinite(time) || !query.client || !query.question?.name) throw new Error('Malformed AdGuard query record');
    if (time < Date.parse(window.since)) continue;
    const query_id = JSON.stringify([query.time, query.client, query.question.class, query.question.name, query.question.type, query.client_proto ?? '']);
    if (known.has(query_id)) continue;
    known.add(query_id);
    rows.push({ json: {
      query_id,
      queried_at: new Date(time).toISOString(),
      client: query.client,
      client_name: query.client_info?.name ?? '',
      domain: query.question.name,
      query_type: query.question.type ?? '',
      status: query.status ?? '',
      reason: query.reason ?? '',
      blocked: /^Filtered/.test(query.reason ?? ''),
      cached: query.cached === true,
      elapsed_ms: Number.isFinite(Number(query.elapsedMs)) ? Number(query.elapsedMs) : null,
      upstream: query.upstream ?? '',
      raw_json: JSON.stringify(query),
    }});
  }
}
return rows;
