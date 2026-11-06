import { expect, test as base, type APIRequestContext, type Page } from "@playwright/test"
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

export type Proj = { id: string; apiKey: string; name: string }

export async function apiCreateProject(request: APIRequestContext, token: string, name: string): Promise<Proj> {
  const slug = `${name.toLowerCase().replace(/[^a-z0-9]+/g, "-")}-${Math.random().toString(36).slice(2, 7)}`
  const res = await request.post("/api/v3/projects", {
    headers: { Authorization: `Bearer ${token}` },
    data: { name, slug },
  })
  expect(res.status(), await res.text()).toBe(201)
  const d = (await res.json()).data
  return { id: d.id, apiKey: d.apiKey, name }
}

/** Public (widget) ticket create; returns the ticket id. */
export async function widgetTicket(
  request: APIRequestContext,
  p: Proj,
  o: { email: string; subject: string; description: string; name?: string },
): Promise<string> {
  const res = await request.post("/api/v3/tickets", {
    headers: { "X-Project-Key": p.apiKey },
    multipart: { projectId: p.id, clientEmail: o.email, clientName: o.name ?? "Cust", subject: o.subject, description: o.description },
  })
  expect(res.status(), await res.text()).toBe(201)
  return (await res.json()).data.id
}

export async function uiLoginAs(page: Page, email: string, password: string) {
  await page.goto("/")
  await page.getByLabel("Email").fill(email)
  await page.getByLabel("Password").fill(password)
  await page.getByRole("button", { name: /sign in/i }).click()
  await expect(page.getByRole("link", { name: /settings/i })).toBeVisible()
}

let ipCounter = 0
const ipBase = 1 + Math.floor(Math.random() * 200)
/**
 * `test` whose browser and request contexts each present a fresh client address (via the trusted
 * proxy's X-Forwarded-For), so the per-IP sign-in limiter (10/min) is exercised by its own specs
 * and not by the suite's many logins.
 */
export const test = base.extend({
  extraHTTPHeaders: async ({}, use) => {
    const n = ++ipCounter
    await use({ "X-Forwarded-For": `10.${ipBase}.${(n >> 8) & 255}.${n & 255}` })
  },
})
export { expect }
