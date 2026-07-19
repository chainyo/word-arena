import {
  Eraser,
  Flag,
  LoaderCircle,
  Replace,
  Send,
  SkipForward,
} from "lucide-react"
import { useState } from "react"

import type { GameMove } from "@/api/types"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"

type ConfirmActionProps = {
  confirmLabel: string
  description: string
  disabled?: boolean
  onConfirm: () => void
  title: string
  trigger: React.ReactElement
  variant?: "default" | "destructive"
}

function ConfirmAction({
  confirmLabel,
  description,
  disabled,
  onConfirm,
  title,
  trigger,
  variant = "default",
}: ConfirmActionProps) {
  const [open, setOpen] = useState(false)
  return (
    <AlertDialog onOpenChange={setOpen} open={open}>
      <AlertDialogTrigger disabled={disabled} render={trigger} />
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            onClick={() => {
              setOpen(false)
              onConfirm()
            }}
            variant={variant}
          >
            {confirmLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

type GameControlsProps = {
  disabled: boolean
  exchangeIds: number[]
  mode: "place" | "exchange"
  onAction: (action: GameMove) => void
  onClear: () => void
  onModeChange: (mode: "place" | "exchange") => void
  pending: boolean
  placementCount: number
  placements: Extract<GameMove, { type: "place" }>["placements"]
}

export function GameControls({
  disabled,
  exchangeIds,
  mode,
  onAction,
  onClear,
  onModeChange,
  pending,
  placementCount,
  placements,
}: GameControlsProps) {
  const locked = disabled || pending
  return (
    <Card className="mt-3" size="sm">
      <CardHeader className="border-b">
        <CardTitle>Turn controls</CardTitle>
        <CardDescription>
          Draft tiles are local; only the referee response commits the turn.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-wrap gap-2">
        {pending ? (
          <Button disabled size="sm">
            <LoaderCircle className="animate-spin motion-reduce:animate-none" />
            Waiting for referee
          </Button>
        ) : null}
        {mode === "place" ? (
          <>
            <ConfirmAction
              confirmLabel="Submit move"
              description={`Submit ${placementCount} staged ${placementCount === 1 ? "tile" : "tiles"}. The board and score will update only after the referee accepts it.`}
              disabled={locked || placementCount === 0}
              onConfirm={() => onAction({ type: "place", placements })}
              title="Play staged tiles?"
              trigger={
                <Button size="sm">
                  <Send /> Play move
                </Button>
              }
            />
            <Button
              disabled={locked}
              onClick={() => onModeChange("exchange")}
              size="sm"
              variant="outline"
            >
              <Replace /> Exchange
            </Button>
          </>
        ) : (
          <>
            <ConfirmAction
              confirmLabel="Exchange tiles"
              description={`Return ${exchangeIds.length} selected ${exchangeIds.length === 1 ? "tile" : "tiles"}. Replacement tiles remain unknown until the referee responds.`}
              disabled={locked || exchangeIds.length === 0}
              onConfirm={() =>
                onAction({ type: "exchange", tile_ids: exchangeIds })
              }
              title="Exchange selected tiles?"
              trigger={
                <Button size="sm">
                  <Replace /> Confirm exchange
                </Button>
              }
            />
            <Button
              disabled={locked}
              onClick={() => onModeChange("place")}
              size="sm"
              variant="outline"
            >
              Cancel exchange
            </Button>
          </>
        )}
        <Button
          disabled={
            locked || (placementCount === 0 && exchangeIds.length === 0)
          }
          onClick={onClear}
          size="sm"
          variant="ghost"
        >
          <Eraser /> Clear
        </Button>
        <ConfirmAction
          confirmLabel="Pass turn"
          description="Passing scores zero points and advances the authoritative turn."
          disabled={locked}
          onConfirm={() => onAction({ type: "pass" })}
          title="Pass this turn?"
          trigger={
            <Button size="sm" variant="outline">
              <SkipForward /> Pass
            </Button>
          }
        />
        <ConfirmAction
          confirmLabel="Resign game"
          description="Resigning immediately finishes the game and cannot be undone."
          disabled={locked}
          onConfirm={() => onAction({ type: "resign" })}
          title="Resign this game?"
          trigger={
            <Button size="sm" variant="destructive">
              <Flag /> Resign
            </Button>
          }
          variant="destructive"
        />
      </CardContent>
    </Card>
  )
}
