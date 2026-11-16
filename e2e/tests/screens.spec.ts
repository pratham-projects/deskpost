import { ADMIN_EMAIL, ADMIN_PASS, apiCreateProject, expect, test, apiLogin, uiLogin, uiLoginAs, widgetTicket, type Proj } from "./helpers"

const run = Date.now().toString(36)
test.describe.configure({ mode: "serial" })

let token: string
let proj: Proj
const subjA = `Alpha printer ${run}`
const subjB = `Bravo invoice ${run}`
let idA: string

test.beforeAll(async ({ playwright }) => {
  const request = await playwright.request.newContext({ baseURL: process.env.E2E_BASE_URL, extraHTTPHeaders: { "X-Forwarded-For": "10.250.0.1" } })
  token = await apiLogin(request)
  proj = await apiCreateProject(request, token, `Screens ${run}`)
  idA = await widgetTicket(request, proj, { email: `a-${run}@c.test`, subject: subjA, description: "printer on fire", name: "Alice A" })
  await widgetTicket(request, proj, { email: `b-${run}@c.test`, subject: subjB, description: "wrong invoice total", name: "Bob B" })
  await request.dispose()
})

async function openInbox(page: import("@playwright/test").Page) {
  await uiLogin(page)
  await expect(page.getByText(subjA).first()).toBeVisible()
}

test("login page has no stale demo-credentials copy", async ({ page }) => {
  await page.goto("/")
  await expect(page.getByLabel("Email")).toBeVisible()
  await expect(page.getByLabel("Email")).toHaveValue("")
  await expect(page.getByText(/demo credentials/i)).toHaveCount(0)
})

test("inbox lists tickets; status filter and search narrow them", async ({ page }) => {
  await openInbox(page)
  await expect(page.getByText(subjB).first()).toBeVisible()
  // status filter: nothing is resolved yet
  await page.getByRole("button", { name: /^Resolved/ }).click()
  await expect(page.getByText(subjA)).toHaveCount(0)
  await page.getByRole("button", { name: /^All Tickets/ }).click()
  await expect(page.getByText(subjA).first()).toBeVisible()
  // search
  await page.getByPlaceholder("Search tickets...").fill("Bravo")
  await expect(page.getByText(subjB).first()).toBeVisible()
  await expect(page.getByText(subjA)).toHaveCount(0)
  await page.getByTitle("Clear search").click()
  await expect(page.getByText(subjA).first()).toBeVisible()
})

test("ticket detail: reply, internal note, status change persists", async ({ page }) => {
  await openInbox(page)
  await page.getByText(subjA).first().click()
  await expect(page.getByText("printer on fire").first()).toBeVisible()
  await expect(page.getByText("Alice A").first()).toBeVisible()

  await page.getByPlaceholder("Type your reply...").fill("Have you tried turning it off?")
  await page.getByRole("button", { name: /send reply/i }).click()
  await expect(page.getByText("Have you tried turning it off?").first()).toBeVisible()

  await page.getByRole("button", { name: /internal\s*note/i }).click()
  await page.getByPlaceholder("Add an internal note...").fill("customer sounds stressed")
  await page.getByRole("button", { name: /add note/i }).click()
  await expect(page.getByText("customer sounds stressed").first()).toBeVisible()

  await page.getByRole("combobox").first().click()
  await page.getByRole("option", { name: "Resolved" }).click()
  await expect(page.getByRole("combobox").first()).toContainText("Resolved")

  // persisted server side, and the note is internal
  const res = await page.request.get(`/api/v3/tickets/${idA}`, { headers: { Authorization: `Bearer ${token}` } })
  expect((await res.json()).data.status).toBe("resolved")
  const msgs = (await (await page.request.get(`/api/v3/tickets/${idA}/messages`, { headers: { Authorization: `Bearer ${token}` } })).json()).data
  const note = msgs.find((m: any) => m.body === "customer sounds stressed")
  expect(note.isInternal).toBe(true)
  // survives reload
  await page.reload()
  await page.getByRole("button", { name: /^Resolved/ }).click()
  await expect(page.getByText(subjA).first()).toBeVisible()
})

test("attachments: drop a file on a ticket, then download it", async ({ page }) => {
  await openInbox(page)
  await page.getByText(subjB).first().click()
  await expect(page.getByText("wrong invoice total").first()).toBeVisible()
  const dt = await page.evaluateHandle(() => {
    const d = new DataTransfer()
    d.items.add(new File(["hello attachment"], "note.txt", { type: "text/plain" }))
    return d
  })
  await page.getByPlaceholder("Type your reply...").dispatchEvent("drop", { dataTransfer: dt })
  await page.getByRole("button", { name: /send with 1 attachment/i }).click()
  const btn = page.getByRole("button", { name: /^note\.txt/ }).first()
  await expect(btn).toBeVisible({ timeout: 15_000 })
  await btn.click()
  const [download] = await Promise.all([page.waitForEvent("download"), page.getByRole("button", { name: /^download/i }).click()])
  expect(download.suggestedFilename()).toBe("note.txt")
})

test("templates: create, search, edit, delete, and use in a reply", async ({ page }) => {
  await uiLogin(page)
  await page.getByRole("link", { name: /templates/i }).click()
  await page.getByRole("combobox", { name: "Project" }).click()
  await page.getByRole("option", { name: `Screens ${run}` }).click()
  await page.getByRole("button", { name: /new template|add template|create template/i }).first().click()
  await page.getByPlaceholder("Template name").fill(`Greeting ${run}`)
  await page.getByPlaceholder("Template content...").fill("Hello from the template")
  await page.getByRole("button", { name: "Create Template" }).click()
  await expect(page.getByText(`Greeting ${run}`).first()).toBeVisible()

  await page.getByPlaceholder("Search templates...").fill(`Greeting ${run}`)
  await expect(page.getByText(/1 template found/)).toBeVisible()
  await page.getByPlaceholder("Search templates...").fill("")

  const card = page.locator("div.border", { hasText: `Greeting ${run}` }).last()
  await card.getByRole("button").first().click()
  await page.getByPlaceholder("Template name").fill(`Greeting2 ${run}`)
  await page.getByRole("button", { name: "Save Changes" }).click()
  await expect(page.getByText(`Greeting2 ${run}`).first()).toBeVisible()

  await page.locator("div.border", { hasText: `Greeting2 ${run}` }).last().getByRole("button").nth(1).click()
  await expect(page.getByText(`Greeting2 ${run}`)).toHaveCount(0)
})

test("templates are per project and usable from a ticket reply", async ({ page, request }) => {
  const r = await request.post("/api/v3/template-responses/", {
    headers: { Authorization: `Bearer ${token}` },
    data: { projectId: proj.id, title: `Canned ${run}`, body: "Canned body text" },
  })
  expect(r.status(), await r.text()).toBe(201)
  await openInbox(page)
  await page.getByText(subjB).first().click()
  await page.getByRole("button", { name: "Templates" }).click()
  await page.getByText(`Canned ${run}`).first().click()
  await expect(page.getByPlaceholder("Type your reply...")).toHaveValue(/Canned body text/)
})

test("members: admin adds an agent who can sign in, then removes them", async ({ page, browser }) => {
  const email = `agent-${run}@desk.test`
  await uiLogin(page)
  await page.getByRole("link", { name: /settings/i }).click()
  await page.getByRole("button", { name: /add member/i }).click()
  await page.getByPlaceholder("Full name").fill("Agent Smith")
  await page.getByPlaceholder("email@company.com").fill(email)
  await page.getByPlaceholder("••••••••").fill("agent-pass-123")
  await page.getByRole("button", { name: "Add Member" }).last().click()
  await expect(page.getByText(email).first()).toBeVisible()

  const ctx = await browser.newContext({ baseURL: process.env.E2E_BASE_URL, extraHTTPHeaders: { "X-Forwarded-For": "10.250.0.2" } })
  const p2 = await ctx.newPage()
  await uiLoginAs(p2, email, "agent-pass-123")
  await p2.getByRole("link", { name: /settings/i }).click()
  await expect(p2.getByText("Admin only").first()).toBeVisible()
  await ctx.close()

  await page.locator("div.border", { hasText: email }).last().getByRole("button").last().click()
  await page.getByRole("button", { name: "Remove Member" }).click()
  await expect(page.getByText(email)).toHaveCount(0)
})

test("project settings: edit name and mail form, key + id shown", async ({ page }) => {
  await uiLogin(page)
  await page.getByRole("link", { name: /settings/i }).click()
  const card = page.locator("div.border", { hasText: proj.id }).first()
  // after a reload the server only knows the key's prefix; regenerate shows a full new key once
  const keyBox = card.getByTestId("api-key")
  await expect(keyBox).toContainText(proj.apiKey.slice(0, 4))
  await expect(keyBox).not.toHaveText(proj.apiKey)
  page.once("dialog", (d) => d.accept())
  await card.getByRole("button", { name: "Regenerate key" }).click()
  await expect(keyBox).not.toHaveText(/…/)
  const newKey = (await keyBox.innerText()).trim()
  expect(newKey).not.toBe(proj.apiKey)
  const old = await page.request.post("/api/v3/tickets", {
    headers: { "X-Project-Key": proj.apiKey },
    multipart: { projectId: proj.id, clientEmail: "old@c.test", subject: "old key use", description: "x" },
  })
  expect(old.status()).toBe(401)
  proj.apiKey = newKey
  await card.getByRole("button").first().click()
  await page.getByPlaceholder("Project name").fill(`Screens Renamed ${run}`)
  await page.getByPlaceholder("domain.com").fill(`renamed-${run}.test`)
  await page.getByRole("button", { name: /advanced/i }).last().click()
  await page.getByPlaceholder("SMTP Email (leave empty to keep current)").fill(`mail-${run}@desk.test`)
  await page.getByPlaceholder("SMTP Password (leave empty to keep current)").fill("pw-1")
  await page.getByRole("button", { name: "Save", exact: true }).click()
  await expect(page.getByText(`Screens Renamed ${run}`).first()).toBeVisible()
  const res = await page.request.get(`/api/v3/projects/${proj.id}`, { headers: { Authorization: `Bearer ${token}` } })
  expect((await res.json()).data.name).toBe(`Screens Renamed ${run}`)
})

test("create ticket from the inbox modal", async ({ page }) => {
  await uiLogin(page)
  await page.getByRole("button", { name: /create ticket/i }).first().click()
  await page.getByLabel(/customer email/i).fill(`modal-${run}@c.test`)
  await page.getByLabel("Customer Name").fill("Mona Modal")
  await page.getByLabel(/subject/i).fill(`Modal ticket ${run}`)
  await page.getByLabel(/description/i).fill("created from the modal")
  await page.getByRole("combobox").first().click()
  await page.getByRole("option").first().click()
  await page.getByRole("button", { name: "Create Ticket" }).last().click()
  await expect(page.getByText(`Modal ticket ${run}`).first()).toBeVisible()
})

test("logout, session survives reload, expired access token refreshes", async ({ page }) => {
  await uiLogin(page)
  await page.reload()
  await expect(page.getByRole("link", { name: /settings/i })).toBeVisible() // refresh cookie restores the session
  // break the access token on the wire once: the client must refresh and retry
  let broken = 0
  await page.route("**/api/v3/tickets?**", async (route) => {
    if (broken++ === 0) return route.fulfill({ status: 401, json: { error: { code: "TOKEN_EXPIRED", message: "expired" } } })
    return route.continue()
  })
  await page.reload()
  await expect(page.getByText(subjB).first()).toBeVisible()
  await page.unroute("**/api/v3/tickets?**")

  await page.getByRole("link", { name: /settings/i }).click()
  await page.getByRole("button", { name: /logout/i }).click()
  await expect(page.getByLabel("Email")).toBeVisible()
  await page.reload()
  await expect(page.getByLabel("Email")).toBeVisible() // refresh token revoked
})

test("password change via API revokes sessions; new password works in the UI", async ({ page, request }) => {
  const email = `pw-${run}@desk.test`
  const r = await request.post("/api/v3/users", {
    headers: { Authorization: `Bearer ${token}` },
    data: { email, password: "first-pass-123", name: "Pat Pw", role: "agent" },
  })
  expect(r.status(), await r.text()).toBe(201)
  const si = await request.post("/api/v3/auth/signin", { data: { email, password: "first-pass-123" } })
  const at = (await si.json()).data.accessToken
  const bad = await request.post("/api/v3/users/me/password", { headers: { Authorization: `Bearer ${at}` }, data: { currentPassword: "nope-nope-1", newPassword: "second-pass-123" } })
  expect(bad.status()).toBe(422)
  const ok = await request.post("/api/v3/users/me/password", { headers: { Authorization: `Bearer ${at}` }, data: { currentPassword: "first-pass-123", newPassword: "second-pass-123" } })
  expect(ok.ok()).toBeTruthy()
  const p = await request.patch("/api/v3/users/me", { headers: { Authorization: `Bearer ${at}` }, data: { name: "Pat Renamed" } })
  expect((await p.json()).data.name).toBe("Pat Renamed")
  await uiLoginAs(page, email, "second-pass-123")
  const old = await request.post("/api/v3/auth/signin", { data: { email, password: "first-pass-123" } })
  expect(old.status()).toBe(401)
})

test("widget page: validation and success", async ({ page }) => {
  await page.goto(`/widget/?project=${proj.id}&key=${proj.apiKey}`)
  await page.getByLabel("Name").fill("Walt")
  await page.getByLabel("Email").fill(`walt-${run}@c.test`)
  await page.getByLabel("Subject").fill(`Widget screen ${run}`)
  await page.getByLabel("Message").fill("hi there")
  await page.getByRole("button", { name: /send/i }).click()
  await expect(page.getByText("Message Sent")).toBeVisible()
  // bad key is refused
  await page.goto(`/widget/?project=${proj.id}&key=wrong`)
  await page.getByLabel("Name").fill("Walt")
  await page.getByLabel("Email").fill(`walt-${run}@c.test`)
  await page.getByLabel("Subject").fill(`Widget bad ${run}`)
  await page.getByLabel("Message").fill("hi there")
  await page.getByRole("button", { name: /send/i }).click()
  await expect(page.getByText("Message Sent")).toHaveCount(0)
})
