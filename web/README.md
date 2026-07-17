# Word Arena web

The live game, replay, tournament, and statistics interface for Word Arena. It
uses Vite, React, Tailwind CSS, and shadcn/ui with Base UI primitives. Bun is
the only supported TypeScript package manager.

## Commands

```bash
bun install
bun run dev
bun run check
bun run format
```

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
