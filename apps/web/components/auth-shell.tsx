import type React from "react"
import { Inbox } from "lucide-react"

/** Centered card used by the signed-out pages (forgot / reset password). */
export function AuthShell({ subtitle, children }: { subtitle: string; children: React.ReactNode }) {
  return (
    <div className="min-h-screen flex items-center justify-center bg-background p-4">
      <div className="w-full max-w-sm">
        <div className="flex flex-col items-center mb-8">
          <div className="w-12 h-12 bg-primary/10 rounded-xl flex items-center justify-center mb-4">
            <Inbox className="w-6 h-6 text-primary" />
          </div>
          <h1 className="text-2xl font-semibold">SendDesk</h1>
          <p className="text-muted-foreground text-sm mt-1">{subtitle}</p>
        </div>
        {children}
      </div>
    </div>
  )
}
