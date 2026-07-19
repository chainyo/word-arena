import { Clock3 } from "lucide-react"
import { useEffect, useRef, useState } from "react"

import { Badge } from "@/components/ui/badge"

type GameClockProps = {
  active: boolean
  deadlineAt?: number
  label: string
  observedAt?: number
}

export function formatClock(remainingMs?: number): string {
  if (remainingMs === undefined) return "--:--"
  const seconds = Math.max(0, Math.ceil(remainingMs / 1_000))
  const minutes = Math.floor(seconds / 60)
  return `${String(minutes).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`
}

export function GameClock({
  active,
  deadlineAt,
  label,
  observedAt,
}: GameClockProps) {
  const mountedAt = useRef(Date.now())
  const [elapsed, setElapsed] = useState(0)
  useEffect(() => {
    if (!active || deadlineAt === undefined || observedAt === undefined) return
    const timer = window.setInterval(
      () => setElapsed(Date.now() - mountedAt.current),
      250
    )
    return () => window.clearInterval(timer)
  }, [active, deadlineAt, observedAt])
  const remainingMs =
    deadlineAt === undefined || observedAt === undefined
      ? undefined
      : deadlineAt - observedAt - elapsed
  const time = formatClock(remainingMs)
  return (
    <Badge
      aria-label={`${label} clock: ${remainingMs === undefined ? "managed by referee" : time}`}
      className="gap-1.5 font-mono tabular-nums"
      variant={active ? "secondary" : "outline"}
    >
      <Clock3 className="size-3.5" /> {time}
    </Badge>
  )
}
