// Default build: next.config.mjs aliases components/demo/mock-mount to this no-op,
// so the mock API, seed data and demo badge are never bundled.
export function MockMount() {
  return null
}
