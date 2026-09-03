import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const SEGMENT_ID = "1709164800000000";
const SAMPLE_TO = "1709164801000000";
const SOURCES = 3;
const [glueArgument, nativeArgument] = process.argv.slice(2);
assert.ok(glueArgument, "generated WebAssembly glue path is required");
assert.ok(nativeArgument, "native oracle path is required");

const gluePath = resolve(glueArgument);
const nativePath = resolve(nativeArgument);
const wasmPath = gluePath.replace(/\.js$/, "_bg.wasm");
const fixtureRoot = new URL("../../../bins/kronika-report/tests/fixtures/", import.meta.url);
const [bindings, wasmBytes, zms, idx] = await Promise.all([
  import(pathToFileURL(gluePath).href),
  readFile(wasmPath),
  readFile(new URL("standalone.zms", fixtureRoot)),
  readFile(new URL("standalone.idx", fixtureRoot)),
]);
const wasm = await bindings.default({ module_or_path: wasmBytes });
const memoryBeforeBytes = wasm.memory.buffer.byteLength;
let cases = 0;
let outputBytes = 0;

function nativeBody(path, query) {
  const result = spawnSync(nativePath, [path, query], {
    encoding: null,
    maxBuffer: 16 * 1024 * 1024,
  });
  assert.equal(
    result.status,
    0,
    `native request failed: ${result.stderr.toString("utf8")}`,
  );
  return result.stdout;
}

function wasmBody(path, query) {
  const response = bindings.request(
    SEGMENT_ID,
    zms,
    idx,
    SOURCES,
    BigInt(zms.length),
    path,
    query,
  );
  assert.equal(response.status, 200);
  assert.equal(response.code, undefined);
  assert.equal(response.parameter, undefined);
  assert.equal(response.message, undefined);
  return Buffer.from(response.takeBody());
}

function compare(name, path, query) {
  const native = nativeBody(path, query);
  const browser = wasmBody(path, query);
  assert.deepEqual(browser, native, `${name} bytes differ`);
  cases += 1;
  outputBytes += browser.length;
  return browser;
}

function records(body) {
  return body
    .toString("utf8")
    .trimEnd()
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

compare("catalog", "/api/catalog", "");
compare(
  "index",
  `/api/segments/${SEGMENT_ID}/sections/pg_stat_database/index`,
  "",
);
compare(
  "hour",
  "/api/hour",
  `from=${SEGMENT_ID}&to=${SAMPLE_TO}&part=base`,
);

const rowsPath = `/api/segments/${SEGMENT_ID}/sections/os_process/rows`;
const firstQuery = "field=comm&field=utime&order=asc&page_size=1";
const firstPage = compare("rows-first-page", rowsPath, firstQuery);
const nextCursor = records(firstPage).find((record) => record.record === "page")
  ?.next_cursor;
assert.equal(typeof nextCursor, "string", "first page must carry a cursor");
compare(
  "rows-next-page",
  rowsPath,
  `${firstQuery}&cursor=${encodeURIComponent(nextCursor)}`,
);

const events = compare(
  "detail-source",
  "/api/events",
  `from=${SEGMENT_ID}&to=1709164801000001&representation=occurrences&limit=5&source=pg_log_errors`,
);
const detailRef = records(events).find(
  (record) => record.record === "event_occurrence",
)?.detail_ref;
assert.equal(typeof detailRef, "string", "event occurrence must carry a detail ref");
compare(
  "row-detail",
  "/api/row-detail",
  `detail_ref=${encodeURIComponent(detailRef)}`,
);

process.stdout.write(
  `${JSON.stringify({
    cases,
    zmsBytes: zms.length,
    idxBytes: idx.length,
    outputBytes,
    memoryBeforeBytes,
    memoryAfterBytes: wasm.memory.buffer.byteLength,
  })}\n`,
);
