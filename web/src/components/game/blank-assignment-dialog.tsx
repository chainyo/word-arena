import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

const letters = Array.from({ length: 26 }, (_, index) =>
  String.fromCharCode(65 + index)
)

type BlankAssignmentDialogProps = {
  onAssign: (letter: string) => void
  onOpenChange: (open: boolean) => void
  open: boolean
}

export function BlankAssignmentDialog({
  onAssign,
  onOpenChange,
  open,
}: BlankAssignmentDialogProps) {
  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Assign the blank tile</DialogTitle>
          <DialogDescription>
            Choose the physical A–Z letter shown on the board. Accented French
            spellings are normalized by the referee.
          </DialogDescription>
        </DialogHeader>
        <fieldset className="grid grid-cols-6 gap-2">
          <legend className="sr-only">Blank letter</legend>
          {letters.map((letter) => (
            <Button
              aria-label={`Use blank as ${letter}`}
              key={letter}
              onClick={() => onAssign(letter)}
              size="icon-sm"
              variant="outline"
            >
              {letter}
            </Button>
          ))}
        </fieldset>
      </DialogContent>
    </Dialog>
  )
}
