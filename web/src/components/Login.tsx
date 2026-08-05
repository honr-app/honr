import { useMemo, useState, type FormEvent } from "react";
import { api } from "../api.js";
import type { AuthStatus } from "../types.js";

export function Login({
  status,
  onAuthed,
}: {
  status: AuthStatus;
  onAuthed: (next: AuthStatus) => void;
}) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const authError = useMemo(() => {
    const q = new URLSearchParams(window.location.search);
    const e = q.get("auth_error");
    if (e === "not_allowlisted") {
      return "Your GitHub account is not on the allowlist.";
    }
    return null;
  }, []);

  const bootstrap = status.bootstrap;
  const submitLabel = bootstrap ? "Create admin & continue" : "Sign in";

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    const body = { username: username.trim(), password };
    const req = bootstrap ? api.bootstrap(body) : api.login(body);
    req
      .then((next) => {
        // Drop auth_error query if present.
        if (window.location.search) {
          window.history.replaceState({}, "", window.location.pathname);
        }
        onAuthed(next);
      })
      .catch((err) => setError(String(err)))
      .finally(() => setBusy(false));
  };

  return (
    <div className="login-shell" data-testid="login">
      <section className="login-card" aria-labelledby="login-title">
        <h1 id="login-title">honr</h1>
        <p className="dim">
          {bootstrap
            ? "First run — set a local admin password. The board stays locked until this is done."
            : "Local admin or Sign in with GitHub (allowlisted users / teams)."}
        </p>

        {(error || authError) && (
          <div className="err" data-testid="login-error">
            {error || authError}
          </div>
        )}

        <form className="login-form" onSubmit={submit} data-testid="login-form">
          <label>
            Username
            <input
              className="search-input"
              autoComplete="username"
              value={username}
              disabled={busy}
              onChange={(e) => setUsername(e.target.value)}
              data-testid="login-username"
            />
          </label>
          <label>
            Password
            <input
              className="search-input"
              type="password"
              autoComplete={bootstrap ? "new-password" : "current-password"}
              value={password}
              disabled={busy}
              onChange={(e) => setPassword(e.target.value)}
              data-testid="login-password"
            />
          </label>
          <div className="btns">
            <button
              type="submit"
              className="primary"
              disabled={busy || !username.trim() || password.length < 8}
              data-testid="login-submit"
            >
              {submitLabel}
            </button>
          </div>
          {!bootstrap && password.length > 0 && password.length < 8 && (
            <p className="dim">Password must be at least 8 characters.</p>
          )}
        </form>

        {!bootstrap && status.github_login_enabled && (
          <div className="login-github" data-testid="login-github">
            <a
              className="btn-link"
              href={`/auth/github?return_origin=${encodeURIComponent(window.location.origin)}`}
            >
              Sign in with GitHub
            </a>
            <p className="dim">
              Only allowlisted GitHub users or org team members can sign in.
            </p>
          </div>
        )}

        {!bootstrap && !status.github_login_enabled && (
          <p className="dim" data-testid="login-github-disabled">
            GitHub login needs Client ID + Client secret on Settings → GitHub App
            (after you sign in as local admin).
          </p>
        )}
      </section>
    </div>
  );
}
