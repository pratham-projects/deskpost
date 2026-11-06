/**
 * API location. NEXT_PUBLIC_API_URL is the API origin; empty (default) means same origin,
 * which is how the static export is served by the API or behind Caddy.
 */
export const API_BASE_URL = (process.env.NEXT_PUBLIC_API_URL ?? "").replace(/\/+$/, "")
export const API_VERSION = process.env.NEXT_PUBLIC_API_VERSION || "v3"
export const API_PREFIX = `${API_BASE_URL}/api/${API_VERSION}`
