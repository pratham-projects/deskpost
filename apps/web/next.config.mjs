const useMock = process.env.NEXT_PUBLIC_USE_MOCK === "1"

/** @type {import('next').NextConfig} */
const nextConfig = {
  // Static export -> out/, served by the API or Caddy.
  output: "export",
  trailingSlash: true,
  images: {
    unoptimized: true,
  },
  // Demo build (NEXT_PUBLIC_USE_MOCK=1) keeps the in-browser mock API; default build swaps in a stub.
  turbopack: {
    resolveAlias: useMock
      ? {}
      : { "@/components/demo/mock-mount": "./components/demo/mock-mount.stub.tsx" },
  },
}

export default nextConfig
