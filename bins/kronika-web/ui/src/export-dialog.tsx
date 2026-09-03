import { Download } from "lucide-react"
import { useEffect, useId, useLayoutEffect, useRef, useState, type FormEvent } from "react"

import type { DisplayTimeZone } from "./display-time"
import { ExportResponseError, fetchExportArtifact, triggerHtmlDownload } from "./export-download"
import { exportRangeDefaults, parseExportRange } from "./export-time"
import type { Translate } from "./help"
import { apiFetch } from "./session"

interface DialogError {
  readonly field: boolean
  readonly message: string
}

const SERVER_ERROR_KEYS = new Set(["bad_parameter", "export_busy", "export_empty", "export_failed"])

export function ExportDialog({ hour, mode, onClose, t }: {
  readonly hour: number
  readonly mode: DisplayTimeZone
  readonly onClose: () => void
  readonly t: Translate
}) {
  const [defaults] = useState(() => exportRangeDefaults(hour, mode))
  const [from, setFrom] = useState(defaults.from)
  const [to, setTo] = useState(defaults.to)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<DialogError | null>(null)
  const dialog = useRef<HTMLDialogElement>(null)
  const fromInput = useRef<HTMLInputElement>(null)
  const opener = useRef<HTMLElement | null>(null)
  const request = useRef<AbortController | null>(null)
  const submitting = useRef(false)
  const titleId = useId()
  const descriptionId = useId()
  const statusId = useId()

  useEffect(() => () => request.current?.abort(), [])
  useLayoutEffect(() => {
    const active = document.activeElement
    if (active instanceof HTMLElement) opener.current = active
    dialog.current?.showModal()
    fromInput.current?.focus({ preventScroll: true })
    return () => {
      if (dialog.current?.open) dialog.current.close()
      const target = opener.current
      requestAnimationFrame(() => {
        if (target?.isConnected) target.focus({ preventScroll: true })
      })
    }
  }, [])

  const dismiss = () => {
    request.current?.abort()
    onClose()
  }
  const changeFrom = (value: string) => {
    setFrom(value)
    setError(null)
  }
  const changeTo = (value: string) => {
    setTo(value)
    setError(null)
  }
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (submitting.current) return
    const range = parseExportRange(from, to, mode, {
      from: defaults.fromSecond,
      to: defaults.toSecond,
    })
    if (!range.ok) {
      setError({ field: true, message: t(`export.error.${range.error}`) })
      return
    }

    submitting.current = true
    setBusy(true)
    setError(null)
    const controller = new AbortController()
    request.current = controller
    try {
      const artifact = await fetchExportArtifact(apiFetch, range.from, range.to, controller.signal)
      if (controller.signal.aborted) return
      triggerHtmlDownload(artifact.blob, artifact.filename)
      onClose()
    } catch (reason) {
      if (!controller.signal.aborted) setError({ field: false, message: requestError(reason, t) })
    } finally {
      if (request.current === controller) request.current = null
      submitting.current = false
      if (!controller.signal.aborted) setBusy(false)
    }
  }

  const describedBy = `${descriptionId} ${statusId}`
  return <dialog
    aria-describedby={descriptionId}
    aria-labelledby={titleId}
    aria-modal="true"
    className="export-dialog fixed inset-0 z-[100] m-auto h-fit max-h-[calc(100dvh-20px)] w-[min(92vw,430px)] overflow-auto rounded-[var(--radius-md)] border border-line3 bg-s1 p-[18px] text-fg shadow-[var(--shadow-pop)]"
    data-testid="export-dialog"
    onCancel={(event) => { event.preventDefault(); dismiss() }}
    onKeyDown={(event) => event.stopPropagation()}
    ref={dialog}
    role="dialog"
  >
    <div className="flex items-start justify-between gap-3 border-b border-line2 pb-3">
      <div>
        <p className="m-0 text-sm text-fg4">HTML</p>
        <h2 className="mt-0.5" id={titleId}>{t("export.title")}</h2>
      </div>
      <button aria-label={t("export.close")} className="icon-button flex-none" onClick={dismiss} type="button">×</button>
    </div>
    <p className="mb-3 mt-3 text-sm leading-[1.5] text-fg3" id={descriptionId}>{t("export.mode", { mode: t(`timezone.${mode}`) })}</p>
    <form aria-busy={busy} className="grid gap-3" data-testid="export-form" noValidate onSubmit={(event) => { void submit(event) }}>
      <label>
        <span className="mb-1.5 block text-sm font-medium text-fg3">{t("export.from")}</span>
        <input
          aria-describedby={describedBy}
          aria-invalid={error?.field || undefined}
          className="h-9 w-full rounded-[var(--radius-sm)] border border-line3 bg-bg px-2.5 font-mono text-sm tabular-nums text-fg outline-none transition-colors focus:border-accent focus:shadow-[0_0_0_1px_var(--color-accent-line)] disabled:cursor-wait disabled:opacity-65"
          data-testid="export-from"
          disabled={busy}
          name="from"
          onChange={(event) => changeFrom(event.target.value)}
          ref={fromInput}
          required
          step={1}
          type="datetime-local"
          value={from}
        />
      </label>
      <label>
        <span className="mb-1.5 block text-sm font-medium text-fg3">{t("export.to")}</span>
        <input
          aria-describedby={describedBy}
          aria-invalid={error?.field || undefined}
          className="h-9 w-full rounded-[var(--radius-sm)] border border-line3 bg-bg px-2.5 font-mono text-sm tabular-nums text-fg outline-none transition-colors focus:border-accent focus:shadow-[0_0_0_1px_var(--color-accent-line)] disabled:cursor-wait disabled:opacity-65"
          data-testid="export-to"
          disabled={busy}
          name="to"
          onChange={(event) => changeTo(event.target.value)}
          required
          step={1}
          type="datetime-local"
          value={to}
        />
      </label>
      <div aria-live="polite" className="flex h-12 items-center overflow-auto" data-testid="export-status" id={statusId}>
        {busy && <span className="flex items-center gap-2 text-sm text-fg3" role="status"><progress aria-hidden="true" className="h-1 w-11" />{t("export.building")}</span>}
        {!busy && error !== null && <p className="m-0 border-l-2 border-warn px-2 py-1 text-sm leading-[1.45] text-fg2" role="alert">{error.message}</p>}
      </div>
      <div className="flex justify-end gap-2 border-t border-line2 pt-3">
        <button className="h-8 cursor-pointer rounded-[var(--radius-sm)] border border-line3 bg-s2 px-3 text-sm font-medium text-fg2 transition-colors hover:bg-s3 hover:text-fg" data-testid="export-cancel" onClick={dismiss} type="button">{t("export.cancel")}</button>
        <button className="inline-flex h-8 cursor-pointer items-center justify-center gap-1.5 rounded-[var(--radius-sm)] border-0 bg-accent px-3 text-sm font-semibold text-bg transition-colors hover:bg-accent2 disabled:cursor-wait disabled:opacity-65" data-testid="export-submit" disabled={busy} type="submit"><Download aria-hidden="true" size={14} />{t("export.submit")}</button>
      </div>
    </form>
  </dialog>
}

function requestError(reason: unknown, t: Translate): string {
  if (!(reason instanceof ExportResponseError)) return t("export.error.unavailable")
  if (reason.code !== null && SERVER_ERROR_KEYS.has(reason.code)) return t(`export.error.${reason.code}`)
  return t("export.error.unavailable")
}
