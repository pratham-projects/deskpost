"use client"

import type React from "react"
import { useEffect, useState } from "react"
import { AlertCircle, Loader2 } from "lucide-react"
import { authApi } from "@/lib/api/auth"
import { AuthShell } from "@/components/auth-shell"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"

export default function ForgotPasswordPage() {
  const [enabled, setEnabled] = useState<boolean | null>(null)
  const [email, setEmail] = useState("")
  const [busy, setBusy] = useState(false)
  const [sent, setSent] = useState(false)
  const [error, setError] = useState("")

  useEffect(() => {
    authApi.config().then((c) => setEnabled(c.passwordResetEnabled))
  }, [])

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError("")
    const res = await authApi.forgotPassword(email.trim())
    setBusy(false)
    if (res.success) setSent(true)
    else setError(res.error?.message ?? "Something went wrong. Try again.")
  }

  if (enabled === null) return <AuthShell subtitle="Reset your password">{null}</AuthShell>

  if (!enabled) {
    return (
      <AuthShell subtitle="Reset your password">
        <p className="text-sm text-muted-foreground text-center">
          Password reset by email is not set up on this server. Ask an administrator to reset it for you.
        </p>
        <p className="text-center text-sm mt-4">
          <a href="/" className="underline">Back to sign in</a>
        </p>
      </AuthShell>
    )
  }

  return (
    <AuthShell subtitle="We will email you a reset link">
      {sent ? (
        <div className="space-y-4 text-center" role="status">
          <p className="text-sm">If an account exists for that address, a reset link is on its way. It works for one hour.</p>
          <a href="/" className="text-sm underline">Back to sign in</a>
        </div>
      ) : (
        <form onSubmit={submit} className="space-y-4">
          <div>
            <Label htmlFor="email">Email</Label>
            <Input
              id="email"
              type="email"
              required
              placeholder="you@example.com"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
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
                Sending...
              </>
            ) : (
              "Send reset link"
            )}
          </Button>
          <p className="text-center text-sm">
            <a href="/" className="underline">Back to sign in</a>
          </p>
        </form>
      )}
    </AuthShell>
  )
}
