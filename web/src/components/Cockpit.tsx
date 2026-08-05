/**
 * Cockpit — primary-nav surface for the ops-seat control plane.
 * Placement only in this slice: session status/actions land in a follow-up.
 */
export function Cockpit() {
  return (
    <div className="cockpit-page" data-testid="cockpit-page">
      <header className="board-hero">
        <h1>Cockpit</h1>
        <p className="board-lede">
          Ops seat control plane: session status, Start / Park / Resume / Stop,
          and the openshell attach command. TTY stays in the terminal — not an
          in-browser emulator.
        </p>
      </header>
    </div>
  );
}
