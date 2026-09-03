import assert from "node:assert/strict"
import test from "node:test"

import {
  ExportResponseError,
  FALLBACK_EXPORT_FILENAME,
  exportFilename,
  exportPath,
  fetchExportArtifact,
  triggerHtmlDownload,
} from "../src/export-download.ts"

test("the export request is same-origin, signed, whole-second HTML", async () => {
  const calls: { input?: RequestInfo | URL, init?: RequestInit }[] = []
  const expected = "kronika-1969-12-31-235959-1970-01-01-000000-utc.html"
  const fetcher = async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ input, init })
    return new Response("<!doctype html><title>Kronika</title>", {
      headers: {
        "Content-Disposition": `attachment; filename="${expected}"`,
        "Content-Type": "text/html",
      },
    })
  }
  const controller = new AbortController()
  const artifact = await fetchExportArtifact(fetcher, -1, 0, controller.signal)
  assert.equal(exportPath(-1, 0), "/api/export?from=-1&to=0")
  assert.equal(calls[0]?.input, "/api/export?from=-1&to=0")
  assert.equal(new Headers(calls[0]?.init?.headers).get("Accept"), "text/html")
  assert.equal(calls[0]?.init?.signal, controller.signal)
  assert.equal(artifact.filename, expected)
  assert.equal(await artifact.blob.text(), "<!doctype html><title>Kronika</title>")
  assert.throws(() => exportPath(0.5, 1), /whole seconds/)
})

test("non-OK response text is read and its typed code is retained", async () => {
  const response = new Response('{"error":"export_empty"}', { status: 404 })
  await assert.rejects(
    fetchExportArtifact(async () => response, 1, 2, new AbortController().signal),
    (reason: unknown) => reason instanceof ExportResponseError
      && reason.message === '{"error":"export_empty"}'
      && reason.code === "export_empty",
  )
})

test("Content-Disposition chooses a safe HTML filename", () => {
  assert.equal(exportFilename('attachment; filename="kronika-range-utc.html"'), "kronika-range-utc.html")
  assert.equal(exportFilename("attachment; filename*=UTF-8''kronika-%D1%82%D0%B5%D1%81%D1%82.html"), "kronika-тест.html")
  for (const disposition of [
    null,
    'attachment; filename="../report.html"',
    'attachment; filename="report.txt"',
    "attachment; filename*=UTF-8''..%2Freport.html",
  ]) assert.equal(exportFilename(disposition), FALLBACK_EXPORT_FILENAME)
})

test("a real download preserves the filename and always revokes its object URL", () => {
  const actions: string[] = []
  const link = {
    download: "",
    hidden: false,
    href: "",
    click: () => actions.push("click"),
    remove: () => actions.push("remove"),
  }
  const documentRef = {
    body: { append: (node: unknown) => { assert.equal(node, link); actions.push("append") } },
    createElement: (name: string) => { assert.equal(name, "a"); return link },
  }
  const urlRef = {
    createObjectURL: (blob: Blob) => { assert.equal(blob.type, "text/html"); actions.push("create"); return "blob:kronika" },
    revokeObjectURL: (url: string) => { assert.equal(url, "blob:kronika"); actions.push("revoke") },
  }
  triggerHtmlDownload(new Blob(["<!doctype html>"], { type: "text/html" }), "kronika-range-utc.html", documentRef as never, urlRef)
  assert.equal(link.download, "kronika-range-utc.html")
  assert.equal(link.href, "blob:kronika")
  assert.equal(link.hidden, true)
  assert.deepEqual(actions, ["create", "append", "click", "remove", "revoke"])
})

test("object URLs are revoked when the synthetic click fails", () => {
  const revoked: string[] = []
  const link = { download: "", hidden: false, href: "", click: () => { throw new Error("blocked") }, remove: () => {} }
  assert.throws(() => triggerHtmlDownload(
    new Blob(),
    "kronika.html",
    { body: { append: () => {} }, createElement: () => link } as never,
    { createObjectURL: () => "blob:kronika", revokeObjectURL: (url) => revoked.push(url) },
  ), /blocked/)
  assert.deepEqual(revoked, ["blob:kronika"])
})
