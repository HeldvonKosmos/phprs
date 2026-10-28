import type { NextConfig } from 'next'

// BENCH3: native packages stay outside the bundle (they load the
// better-sqlite3 addon built against SQLite 3.45.2 and bcrypt's binding)
const nextConfig: NextConfig = {
  serverExternalPackages: ['better-sqlite3', 'bcrypt'],
  poweredByHeader: false,
}

export default nextConfig
