import { expect, type APIRequestContext, type Page } from "@playwright/test"
import { ImapFlow } from "imapflow"
import nodemailer from "nodemailer"

export const ADMIN_EMAIL = process.env.E2E_ADMIN_EMAIL!
export const ADMIN_PASS = process.env.E2E_ADMIN_PASS!
export const SMTP_PORT = Number(process.env.E2E_SMTP_PORT)
export const IMAP_PORT = Number(process.env.E2E_IMAP_PORT)

export async function apiLogin(request: APIRequestContext): Promise<string> {
  const res = await request.post("/api/v3/auth/signin", { data: { email: ADMIN_EMAIL, password: ADMIN_PASS } })
  expect(res.ok()).toBeTruthy()
  return (await res.json()).data.accessToken
}

export async function uiLogin(page: Page) {
  await page.goto("/")
  await page.getByLabel("Email").fill(ADMIN_EMAIL)
  await page.getByLabel("Password").fill(ADMIN_PASS)
  await page.getByRole("button", { name: /sign in/i }).click()
  await expect(page.getByRole("link", { name: /settings/i })).toBeVisible()
}

/** Sends a mail to the GreenMail SMTP port as an outside customer. */
export async function sendCustomerMail(opts: { from: string; to: string; subject: string; text: string; headers?: Record<string, string> }) {
  const t = nodemailer.createTransport({ host: "127.0.0.1", port: SMTP_PORT, secure: false, ignoreTLS: true })
  await t.sendMail(opts)
}

/** Polls a GreenMail mailbox until a message with this subject (substring) shows up. */
export async function waitForMail(address: string, subjectPart: string, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const c = new ImapFlow({ host: "127.0.0.1", port: IMAP_PORT, secure: false, auth: { user: address, pass: "x" }, logger: false })
    try {
      await c.connect()
      const lock = await c.getMailboxLock("INBOX")
      try {
        if (c.mailbox && c.mailbox.exists > 0) {
          for await (const m of c.fetch("1:*", { envelope: true, source: true })) {
            if (m.envelope?.subject?.includes(subjectPart)) return { subject: m.envelope.subject, source: m.source!.toString() }
          }
        }
      } finally {
        lock.release()
      }
    } catch {
      /* empty mailbox or not ready */
    } finally {
      await c.logout().catch(() => {})
    }
    await new Promise((r) => setTimeout(r, 1000))
  }
  throw new Error(`no mail "${subjectPart}" for ${address}`)
}
