# Word Arena web

The local game, replay, tournament, and statistics workspace for Word Arena. It
opens directly into the game interface; this repository does not maintain a
separate marketing landing page. The app uses Vite, React, Tailwind CSS, and
shadcn/ui with Base UI primitives. Bun is the only supported TypeScript package
manager.

## Commands

```bash
bun install
bun run dev
bun run check
bun run test
bun run format
bun run fix
```

Biome owns frontend formatting, linting, and import organization. Use `format`
for formatting-only changes and `fix` to apply all safe Biome fixes.

## Live API policy

The default route is the active-game connection workspace. After connection,
routes have the shape `/games/:gameId/:authority`, where authority is `public`,
`seat`, or `spectator`. Capabilities are held only in memory: never put them in
URLs, Web Storage, IndexedDB, service-worker caches, logs, or query keys. A page
reload intentionally requires re-entry.

TanStack Query caches decoded authoritative snapshots. WebSocket messages are
public invalidation markers only; they trigger a fresh authority-specific REST
snapshot instead of mutating game state locally. See [`docs/WEB.md`](../docs/WEB.md).

## Adding components

Always add general-purpose UI through the current shadcn CLI:

```bash
bunx --bun shadcn@latest add <component>
```

Generated primitives live in `src/components/ui`. Game-specific components
should compose them rather than introduce a second UI system.

```tsx
import { Button } from "@/components/ui/button"
```
