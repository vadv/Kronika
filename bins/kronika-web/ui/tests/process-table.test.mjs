import assert from "node:assert/strict"
import test from "node:test"

import { importModule } from "./import-module.mjs"

const { LENS_FIELDS } = await importModule('export { LENS_FIELDS } from "../src/process-table.tsx"')

test("process lenses keep identity first, lens metrics next, and state last", () => {
  const fields = (lens) => LENS_FIELDS[lens].map(({ id }) => id)
  assert.deepEqual(fields("generic"), [
    "pid", "command", "ppid", "uid", "euid", "gid", "egid", "num_threads", "tty", "exit_signal", "state",
  ])
  assert.deepEqual(fields("cpu"), [
    "pid", "command", "utime", "stime", "rundelay_ns", "blkdelay_ticks", "nvcsw", "nivcsw",
    "curcpu", "nice", "prio", "rtprio", "policy", "state",
  ])
  assert.deepEqual(fields("memory"), [
    "pid", "command", "rmem_kb", "vmem_kb", "vswap_kb", "minflt", "majflt", "state",
  ])
  assert.deepEqual(fields("disk"), [
    "pid", "command", "read_bytes", "write_bytes", "syscr", "syscw", "rchar", "wchar",
    "cancelled_write_bytes", "blkdelay_ticks", "state",
  ])
})
