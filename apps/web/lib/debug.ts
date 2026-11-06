/** Dev-only logging. No-op in production builds; never pass tokens or headers. */
export const debug: (...args: unknown[]) => void =
  process.env.NODE_ENV === "production" ? () => {} : (...args) => console.log(...args) // eslint-disable-line no-console
