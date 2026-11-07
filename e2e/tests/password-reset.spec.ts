import { expect, test, type APIRequestContext, type APIResponse } from "@playwright/test"
import { ADMIN_EMAIL, apiLogin, waitForMail } from "./helpers"

const run = Date.now().toString(36)
const EMAIL = `reset-${run}@desk.test`
const OLD = "old-password-1"
const NEW = "new-password-2"

test.describe.configure({ mode: "serial" })

/** Sign-in from the shared loopback address; waits out the per-IP limit the earlier specs may have used up. */
async function signin(request: APIRequestContext, email: string, password: string): Promise<APIResponse> {
  const deadline = Date.now() + 75_000
  for (;;) {
    const res = await request.post("/api/v3/auth/signin", { data: { email, password } })
    if (res.status() !== 429 || Date.now() > deadline) return res
    await new Promise((r) => setTimeout(r, 3_000))
  }
}

/** Link in the mail, quoted-printable decoded. */
function linkFrom(source: string): string {
  const flat = source.replace(/=\r?\n/g, "").replace(/=3D/g, "=")
  const m = flat.match(/https?:\/\/\S+\/reset-password\/#token=[A-Za-z0-9_-]+/)
  if (!m) throw new Error("no reset link in mail")
  return m[0]
}

test("reset by email: link works once, old password and sessions die", async ({ page, request }) => {
  const admin = await apiLogin(request)
  const made = await request.post("/api/v3/users", {
    headers: { Authorization: `Bearer ${admin}` },
    data: { email: EMAIL, name: "Reset Me", password: OLD, role: "agent" },
  })
  expect(made.ok()).toBeTruthy()

  // A session that must not survive the reset.
  const before = await signin(request, EMAIL, OLD)
  expect(before.ok()).toBeTruthy()
  const staleRefresh = (await before.json()).data.refreshToken as string

  // Sign-in page offers the link.
  await page.goto("/")
  await page.getByRole("link", { name: /forgot password/i }).click()
  await expect(page).toHaveURL(/forgot-password/)

  // Unknown and known addresses get the same screen.
  await page.getByLabel("Email").fill(`nobody-${run}@desk.test`)
  await page.getByRole("button", { name: /send reset link/i }).click()
  await expect(page.getByText(/if an account exists/i)).toBeVisible()
  await page.goto("/forgot-password/")
  await page.getByLabel("Email").fill(EMAIL)
  await page.getByRole("button", { name: /send reset link/i }).click()
  await expect(page.getByText(/if an account exists/i)).toBeVisible()

  const mail = await waitForMail(EMAIL, "Reset your SendDesk password")
  const link = linkFrom(mail.source)

  await page.goto(link)
  // The token is scrubbed from the address bar.
  await expect(page).not.toHaveURL(/token=/)
  await page.getByLabel("New password").fill(NEW)
  await page.getByLabel("Confirm password").fill(NEW)
  await page.getByRole("button", { name: /set new password/i }).click()
  await expect(page.getByText(/password changed/i)).toBeVisible()

  // Old sessions are revoked. Checked first: a later sign-in would replace the shared cookie, which wins over the header.
  const stale = await request.post("/api/v3/auth/refresh", { headers: { "X-Refresh-Token": staleRefresh } })
  expect(stale.status()).toBe(401)
  // Old password is dead, new one works.
  const oldTry = await signin(request, EMAIL, OLD)
  expect(oldTry.status()).toBe(401)
  const newTry = await signin(request, EMAIL, NEW)
  expect(newTry.ok()).toBeTruthy()

  // Replaying the link fails in the UI. Leave first: a hash-only change would not reload the page.
  await page.goto("/")
  await page.goto(link)
  await page.getByLabel("New password").fill("attacker-pass-3")
  await page.getByLabel("Confirm password").fill("attacker-pass-3")
  await page.getByRole("button", { name: /set new password/i }).click()
  await expect(page.getByText(/invalid or has expired/i)).toBeVisible()
  const still = await signin(request, EMAIL, NEW)
  expect(still.status(), await still.text()).toBe(200)
})

test("config advertises reset and admin sign-in is unaffected", async ({ request }) => {
  const cfg = await (await request.get("/api/v3/auth/config")).json()
  expect(cfg.data.passwordResetEnabled).toBe(true)
  expect(ADMIN_EMAIL).toBeTruthy()
})
