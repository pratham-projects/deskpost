import { expect, test } from "@playwright/test"
import { readFileSync } from "node:fs"
import { join } from "node:path"

// Contract: every type the UI offers must be accepted by the server (UI list is a subset).
const root = join(__dirname, "..", "..")
const quoted = (src: string) => [...src.matchAll(/["']([a-z]+\/[A-Za-z0-9.+-]+)["']/g)].map((m) => m[1])

test("UI attachment types are a subset of the server allow-list", () => {
  const rust = readFileSync(join(root, "crates/api/src/routes/attachments.rs"), "utf8")
  const fn = rust.slice(rust.indexOf("fn upload_type_allowed"), rust.indexOf("struct Upload"))
  const server = quoted(fn).filter((t) => !t.endsWith("."))
  const ts = readFileSync(join(root, "apps/web/lib/api/attachments.ts"), "utf8")
  const ui = quoted(ts.slice(ts.indexOf("ALLOWED_IMAGE_TYPES"), ts.indexOf("ALLOWED_TYPE_PREFIXES")))
  expect(ui.length).toBeGreaterThan(5)
  for (const t of ui) expect(server, t).toContain(t)
})
