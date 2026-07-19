export type RouteAuthority =
  | "local_operator"
  | "public_observer"
  | "competitive_seat"
  | "human_spectator"

export const ROUTE_REQUIREMENTS = [
  { pattern: /^\/$|^\/operator$|^\/tournaments$/, authority: "local_operator" },
  { pattern: /^\/connect$/, authority: "public_observer" },
  {
    pattern: /^\/games\/[^/]+\/(player|seat)$/,
    authority: "competitive_seat",
  },
  {
    pattern: /^\/games\/[^/]+\/(spectator|replay)$/,
    authority: "human_spectator",
  },
  {
    pattern: /^\/games\/[^/]+\/public$/,
    authority: "public_observer",
  },
  {
    pattern: /^\/tournaments\/[^/]+\/standings$|^\/agents\/[^/]+$/,
    authority: "public_observer",
  },
] as const satisfies ReadonlyArray<{
  pattern: RegExp
  authority: RouteAuthority
}>

export function routeAuthority(pathname: string): RouteAuthority | undefined {
  return ROUTE_REQUIREMENTS.find(({ pattern }) => pattern.test(pathname))
    ?.authority
}
