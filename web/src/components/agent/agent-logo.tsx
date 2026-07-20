import type { AgentHarnessId } from "@/api/types"
import { cn } from "@/lib/utils"

export function AgentLogo({
  agent,
  className,
}: {
  agent: AgentHarnessId
  className?: string
}) {
  if (agent === "pi") {
    return (
      <span
        aria-hidden="true"
        className={cn(
          "grid size-9 place-items-center rounded-xl border bg-background font-heading text-lg font-semibold",
          className
        )}
      >
        π
      </span>
    )
  }

  return (
    <span
      aria-hidden="true"
      className={cn(
        "grid size-9 place-items-center rounded-xl border bg-background",
        className
      )}
    >
      <svg
        className="size-5"
        fill="none"
        viewBox="0 0 24 24"
        xmlns="http://www.w3.org/2000/svg"
      >
        <title>{agent} logo</title>
        {agent === "codex" ? (
          <>
            <path
              d="M12 3.25a4.2 4.2 0 0 1 4.08 3.18 4.2 4.2 0 0 1 2.7 6.4 4.2 4.2 0 0 1-4.5 5.92A4.2 4.2 0 0 1 7.1 17.7a4.2 4.2 0 0 1-1.18-7.13A4.2 4.2 0 0 1 12 3.25Z"
              stroke="currentColor"
              strokeWidth="1.6"
            />
            <path
              d="m8.1 9.35 3.9-2.2 3.9 2.2v4.5l-3.9 2.2-3.9-2.2v-4.5Z"
              stroke="currentColor"
              strokeWidth="1.6"
            />
          </>
        ) : agent === "claude_code" ? (
          <path
            d="M12 3v18M3 12h18M5.64 5.64l12.72 12.72M18.36 5.64 5.64 18.36M8.55 3.75l6.9 16.5M3.75 8.55l16.5 6.9M15.45 3.75l-6.9 16.5M20.25 8.55l-16.5 6.9"
            stroke="currentColor"
            strokeLinecap="round"
            strokeWidth="1.45"
          />
        ) : (
          <path
            d="M18.25 6.5A8 8 0 1 0 18.25 17.5M18 6.5h-5.25M18 17.5h-5.25"
            stroke="currentColor"
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth="2"
          />
        )}
      </svg>
    </span>
  )
}
