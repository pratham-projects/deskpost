import { expect, test } from "@playwright/test"
import { ADMIN_EMAIL, IMAP_PORT, SMTP_PORT, apiLogin, sendCustomerMail, uiLogin, waitForMail } from "./helpers"

const run = Date.now().toString(36)
const SUPPORT = `support-${run}@desk.test`

test.describe.configure({ mode: "serial" })

let project: { id: string; apiKey: string }

test("bad login is rejected, good login lands in the inbox", async ({ page }) => {
  await page.goto("/")
  await page.getByLabel("Email").fill(ADMIN_EMAIL)
  await page.getByLabel("Password").fill("wrong-password-1")
  await page.getByRole("button", { name: /sign in/i }).click()
  await expect(page.getByRole("link", { name: /settings/i })).toHaveCount(0)
  await uiLogin(page)
})

test("create a project in the UI", async ({ page }) => {
  await uiLogin(page)
  await page.getByRole("link", { name: /settings/i }).click()
  await page.getByRole("button", { name: /add project/i }).click()
  await page.getByPlaceholder("My Website").fill(`UI Project ${run}`)
  await page.getByPlaceholder("example.com").fill(`ui-${run}.test`)
  await page.getByPlaceholder("your-email@gmail.com").fill(`ui-${run}@desk.test`)
  await page.getByPlaceholder("••••••••••••••••").fill("smtp-pass")
  await page.getByRole("button", { name: /advanced options/i }).click()
  await page.getByPlaceholder("smtp.gmail.com").fill("127.0.0.1")
  await page.getByPlaceholder("587").fill(String(SMTP_PORT))
  await page.getByRole("button", { name: "Create Project" }).click()
  await expect(page.getByText(`UI Project ${run}`).first()).toBeVisible()
})

test("setup mail project through the API", async ({ request }) => {
  const token = await apiLogin(request)
  const res = await request.post("/api/v3/projects", {
    headers: { Authorization: `Bearer ${token}` },
    data: {
      name: `Mail ${run}`,
      slug: `mail-${run}`,
      smtpEmail: SUPPORT,
      smtpUsername: SUPPORT,
      smtpPassword: "x",
      smtpHost: "127.0.0.1",
      smtpPort: SMTP_PORT,
      imapHost: "127.0.0.1",
      imapPort: IMAP_PORT,
      imapUsername: SUPPORT,
      imapPassword: "x",
    },
  })
  expect(res.status(), await res.text()).toBe(201)
  const d = (await res.json()).data
  project = { id: d.id, apiKey: d.apiKey }
})

test("widget ticket -> agent reply -> customer gets mail", async ({ page }) => {
  const customer = `widget-${run}@customer.test`
  await page.goto(`/widget/?project=${project.id}&key=${project.apiKey}`)
  await page.getByLabel("Name").fill("Wanda Widget")
  await page.getByLabel("Email").fill(customer)
  await page.getByLabel("Subject").fill(`Widget issue ${run}`)
  await page.getByLabel("Message").fill("My widget is broken")
  await page.getByRole("button", { name: /send/i }).click()
  await expect(page.getByText("Message Sent")).toBeVisible()

  await uiLogin(page)
  await page.getByText(`Widget issue ${run}`).first().click()
  await page.getByPlaceholder("Type your reply...").fill("We are on it, Wanda")
  await page.getByRole("button", { name: /send reply/i }).click()
  await expect(page.getByText("We are on it, Wanda").first()).toBeVisible()
  const mail = await waitForMail(customer, "[Ticket #1]")
  expect(mail.source).toContain("We are on it, Wanda")
})

test("mail round trip: email in -> ticket -> reply -> email out", async ({ page }) => {
  const customer = `mailer-${run}@customer.test`
  const subject = `Mail issue ${run}`
  await sendCustomerMail({ from: customer, to: SUPPORT, subject, text: "Please help by email" })

  await uiLogin(page)
  await expect(async () => {
    await page.reload()
    await expect(page.getByText(subject).first()).toBeVisible({ timeout: 2000 })
  }).toPass({ timeout: 40_000, intervals: [3000] })
  await page.getByText(subject).first().click()
  await expect(page.getByText("Please help by email").first()).toBeVisible()
  await page.getByPlaceholder("Type your reply...").fill("Replying by mail now")
  await page.getByRole("button", { name: /send reply/i }).click()
  const mail = await waitForMail(customer, "[Ticket #2]")
  expect(mail.source).toContain("Replying by mail now")
})
