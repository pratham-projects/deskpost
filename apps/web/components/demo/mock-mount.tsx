"use client"

import { installMockApi } from "@/mock/install"
import { DemoBadge } from "./demo-badge"

// Demo build only (NEXT_PUBLIC_USE_MOCK=1, see app/layout.tsx). Runs at import time,
// before any child effect calls tryAutoLogin().
installMockApi()

export function MockMount() {
  return <DemoBadge />
}
