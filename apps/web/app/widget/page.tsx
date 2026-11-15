"use client"

import { Suspense } from "react"
import { useSearchParams } from "next/navigation"
import { WidgetForm } from "@/components/widget-form"

// Embed as /widget?project=<project id>&key=<project api key> (static export: no dynamic route).
function Widget() {
  const params = useSearchParams()
  const projectId = params.get("project")
  const projectKey = params.get("key")
  if (!projectId || !projectKey) {
    return <p className="text-center text-sm text-muted-foreground">Widget needs ?project=&amp;key= parameters.</p>
  }
  return <WidgetForm projectId={projectId} projectKey={projectKey} />
}

export default function WidgetPage() {
  return (
    <div className="min-h-screen bg-background flex items-center justify-center p-4">
      <div className="w-full max-w-md">
        <div className="bg-card border border-border rounded-lg p-6">
          <div className="text-center mb-6">
            <h2 className="text-xl font-semibold">Contact Deskpost</h2>
            <p className="text-sm text-muted-foreground mt-1">We typically respond within 24 hours</p>
          </div>
          <Suspense fallback={null}>
            <Widget />
          </Suspense>
        </div>
      </div>
    </div>
  )
}
