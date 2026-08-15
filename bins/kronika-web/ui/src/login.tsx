import { Activity, LockKeyhole } from "lucide-react"
import { useEffect, useRef, useState, type FormEvent } from "react"

import type { Translate } from "./help"
import type { Locale } from "./model"
import { signInBasic } from "./session"

export function Login({ expired, locale, onLocale, t }: {
  readonly expired: boolean
  readonly locale: Locale
  readonly onLocale: (locale: Locale) => void
  readonly t: Translate
}) {
  const [user, setUser] = useState("")
  const [password, setPassword] = useState("")
  const [error, setError] = useState<"invalid" | "unavailable" | null>(null)
  const [busy, setBusy] = useState(false)
  const request = useRef<AbortController | null>(null)
  useEffect(() => () => request.current?.abort(), [])

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    request.current?.abort()
    const controller = new AbortController()
    request.current = controller
    setBusy(true)
    setError(null)
    setPassword("")
    try {
      const result = await signInBasic(user, password, controller.signal)
      if (controller.signal.aborted) return
      if (result === "invalid") {
        setError("invalid")
      }
    } catch {
      if (!controller.signal.aborted) {
        setError("unavailable")
      }
    } finally {
      if (!controller.signal.aborted) setBusy(false)
    }
  }

  const message = error === null ? expired ? t("auth.expired") : null : t(`auth.${error}`)
  return <main className="login-shell">
    <section className="login-card">
      <header>
        <span className="login-mark"><Activity aria-hidden="true" size={18} /></span>
        <strong>{t("app.title")}</strong>
        <div aria-label={t("locale.switch")} className="locale-switch" role="group">
          {(["ru", "en"] as const).map((choice) => <button aria-pressed={locale === choice} key={choice} onClick={() => onLocale(choice)} type="button">{t(`locale.${choice}`)}</button>)}
        </div>
      </header>
      <div className="login-title"><LockKeyhole aria-hidden="true" size={20} /><div><h1>{t("auth.title")}</h1><p>{t("auth.subtitle")}</p></div></div>
      <form autoComplete="off" onSubmit={(event) => { void submit(event) }}>
        <label><span>{t("auth.user")}</span><input autoCapitalize="none" autoComplete="username" autoFocus disabled={busy} name="username" onChange={(event) => setUser(event.target.value)} required spellCheck={false} type="text" value={user} /></label>
        <label><span>{t("auth.password")}</span><input autoComplete="current-password" disabled={busy} name="password" onChange={(event) => setPassword(event.target.value)} required type="password" value={password} /></label>
        {message !== null && <p aria-live="polite" className="login-message">{message}</p>}
        <button disabled={busy} type="submit">{t(busy ? "auth.signing_in" : "auth.submit")}</button>
      </form>
    </section>
  </main>
}
