import type { Seat } from "@/api/types"

export const SEATS: readonly Seat[] = ["one", "two", "three", "four"]

export function seatLabel(seat: Seat): string {
  return `Seat ${seat}`
}

export function languageLabel(language: "english" | "french"): string {
  return language === "english" ? "🇬🇧 English" : "🇫🇷 Français"
}

export function rulesetLabel(ruleset: "english-v1" | "french-v1"): string {
  return ruleset === "english-v1" ? "🇬🇧 English v1" : "🇫🇷 Français v1"
}

export const seatColorClasses: Record<Seat, string> = {
  one: "border-seat-one/35 bg-seat-one/15",
  two: "border-seat-two/35 bg-seat-two/15",
  three: "border-seat-three/35 bg-seat-three/15",
  four: "border-seat-four/35 bg-seat-four/15",
}

export const activeSeatColorClasses: Record<Seat, string> = {
  one: "bg-seat-one/30",
  two: "bg-seat-two/30",
  three: "bg-seat-three/30",
  four: "bg-seat-four/30",
}

export const seatBorderClasses: Record<Seat, string> = {
  one: "border-l-seat-one/65",
  two: "border-l-seat-two/65",
  three: "border-l-seat-three/65",
  four: "border-l-seat-four/65",
}

export const seatRingClasses: Record<Seat, string> = {
  one: "ring-seat-one/45",
  two: "ring-seat-two/45",
  three: "ring-seat-three/45",
  four: "ring-seat-four/45",
}
