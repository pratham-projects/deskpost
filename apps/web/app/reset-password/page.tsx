"use client"

import type React from "react"
import { useEffect, useState } from "react"
import { AlertCircle, Loader2 } from "lucide-react"
import { authApi } from "@/lib/api/auth"
import { AuthShell } from "@/components/auth-shell"
import { Button } from "@/components/ui/button"
import { Label } from "@/components/ui/label"
import { PasswordInput } from "@/components/ui/password-input"

export default function ResetPasswordPage() {
  const [token, setToken] = useState<string | null>(null)
  const [password, setPassword] = useState("")
  const [confirm, setConfirm] = useState("")
  const [busy, setBusy] = useState(false)
  const [done, setDone] = useState(false)
  const [error, setError] = useState("")

  // The token comes in the URL fragment (never sent to a server). Keep it in memory and scrub the address bar.
  useEffect(() => {
    const t = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("token") ?? ""
    setToken(t)
    if (t) window.history.replaceState(null, "", window.location.pathname)
  }, [])

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setError("")
    if (password.length < 8 || password.length > 128) {
      setError("Password must be 8 to 128 characters.")
      return
    }
    if (password !== confirm) {
      setError("The two passwords do not match.")
      return
    }
    setBusy(true)
    const res = await authApi.resetPassword(token ?? "", password)
    setBusy(false)
    if (res.success) setDone(true)
    else setError(res.error?.message ?? "Something went wrong. Try again.")
  }

  if (token === null) return <AuthShell subtitle="Choose a new password">{null}</AuthShell>

  if (done) {
    return (
      <AuthShell subtitle="Choose a new password">
        <div className="space-y-4 text-center" role="status">
          <p className="text-sm">Password changed. You were signed out everywhere.</p>
          <a href="/" className="text-sm underline">Sign in</a>
        </div>
      </AuthShell>
    )
  }

  if (!token) {
    return (
      <AuthShell subtitle="Choose a new password">
        <p className="text-sm text-muted-foreground text-center">
          This reset link is missing its token. Open the link from the email again, or{" "}
          <a href="/forgot-password/" className="underline">request a new one</a>.
        </p>
      </AuthShell>
    )
  }

  return (
    <AuthShell subtitle="Choose a new password">
      <form onSubmit={submit} className="space-y-4">
        <div>
          <Label htmlFor="password">New password</Label>
          <PasswordInput
            id="password"
            autoComplete="new-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            className="mt-2"
          />
        </div>
        <div>
          <Label htmlFor="confirm">Confirm password</Label>
          <PasswordInput
            id="confirm"
            autoComplete="new-password"
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
            className="mt-2"
          />
        </div>
        {error && (
          <div className="flex items-center gap-2 text-sm text-destructive" role="alert">
            <AlertCircle className="w-4 h-4" />
            {error}
          </div>
        )}
        <Button type="submit" className="w-full" disabled={busy}>
          {busy ? (
            <>
              <Loader2 className="w-4 h-4 mr-2 animate-spin" />
              Saving...
            </>
          ) : (
            "Set new password"
          )}
        </Button>
        <p className="text-center text-sm">
          <a href="/forgot-password/" className="underline">Request a new link</a>
        </p>
      </form>
    </AuthShell>
  )
}
