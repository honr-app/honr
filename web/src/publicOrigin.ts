/** Browser-facing origin for copy/paste URLs (MCP, OAuth examples). */
export function publicOrigin(
  loc: Pick<Location, "origin"> | undefined = typeof window !== "undefined"
    ? window.location
    : undefined,
): string {
  return loc?.origin?.replace(/\/$/, "") ?? "";
}

/** Streamable HTTP MCP endpoint on the origin the operator opened. */
export function publicMcpUrl(
  loc: Pick<Location, "origin"> | undefined = typeof window !== "undefined"
    ? window.location
    : undefined,
): string {
  const origin = publicOrigin(loc);
  return origin ? `${origin}/mcp` : "/mcp";
}
