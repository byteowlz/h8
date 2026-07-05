"""Google OAuth via google-auth: silent refresh plus interactive login.

Credentials are stored as authorized-user JSON through
:class:`~h8.oauth.store.TokenStore` under the key ``google:{email}``.
:func:`get_google_credentials` loads them, refreshes on expiry when a refresh
token is present, re-persists, and raises :class:`~h8.oauth.LoginRequired` when
credentials are absent or unusable.

The Google trap (READ THIS): while the OAuth consent screen is in **Testing**
status, Google issues refresh tokens that **expire after 7 days**, silently
breaking background refresh. Publish the OAuth app to **Production** (or add the
account as a verified test user won't help -- Testing tokens still expire) so
refresh tokens are long-lived. This is the single most common cause of "it
worked last week and now asks me to log in again".

``client_id`` and ``client_secret`` are required for Google and are read from
the account config (``client_id`` attribute and ``extra["client_secret"]``);
the installed-app "secret" is not confidential.
"""

import json
import logging
import threading
import uuid
from dataclasses import dataclass
from typing import Optional

from google.auth.transport.requests import Request
from google.oauth2.credentials import Credentials
from google_auth_oauthlib.flow import Flow, InstalledAppFlow

from h8.oauth import LoginRequired
from h8.oauth._account import AccountLike
from h8.oauth.store import TokenStore, get_default_store

log = logging.getLogger(__name__)

SCOPES = [
    "https://www.googleapis.com/auth/gmail.modify",
    "https://www.googleapis.com/auth/gmail.settings.basic",
    "https://www.googleapis.com/auth/calendar",
    "https://www.googleapis.com/auth/contacts",
]

_AUTH_URI = "https://accounts.google.com/o/oauth2/auth"
_TOKEN_URI = "https://oauth2.googleapis.com/token"
# Loopback redirect for the manual URL-paste (headless) flow.
_HEADLESS_REDIRECT_URI = "http://localhost"

_sessions: dict[str, dict] = {}
_sessions_lock = threading.Lock()


@dataclass
class LoginSession:
    """Interactive login handle. ``auth_url`` is set for the headless flow."""

    session_id: str
    auth_url: Optional[str]


def _cred_key(email: str) -> str:
    return f"google:{email}"


def _get_store() -> TokenStore:
    return get_default_store()


def _client_config(account: AccountLike) -> dict:
    """Build an installed-app client config from the account, or raise."""
    client_id = (account.client_id or "").strip()
    client_secret = str((account.extra or {}).get("client_secret", "")).strip()
    if not client_id or not client_secret:
        raise LoginRequired(
            f"Google account '{account.email}' requires `client_id` and "
            "`client_secret`. Create OAuth client credentials (type: Desktop "
            "app) in Google Cloud Console and set them in the account config."
        )
    return {
        "installed": {
            "client_id": client_id,
            "client_secret": client_secret,
            "auth_uri": _AUTH_URI,
            "token_uri": _TOKEN_URI,
            "redirect_uris": [_HEADLESS_REDIRECT_URI],
        }
    }


def get_google_credentials(account: AccountLike) -> Credentials:
    """Return valid Google credentials, refreshing and re-persisting as needed.

    Raises :class:`~h8.oauth.LoginRequired` when no stored credentials exist or
    they are invalid and cannot be refreshed.
    """
    store = _get_store()
    raw = store.get(_cred_key(account.email))
    if not raw:
        raise LoginRequired(
            f"No stored Google credentials for '{account.email}'. Run the login flow."
        )

    try:
        creds = Credentials.from_authorized_user_info(json.loads(raw), SCOPES)
    except (ValueError, KeyError, json.JSONDecodeError) as exc:
        raise LoginRequired(
            f"Stored Google credentials for '{account.email}' are unreadable: {exc}"
        ) from exc

    if creds.expired and creds.refresh_token:
        try:
            creds.refresh(Request())
        except Exception as exc:  # noqa: BLE001 - refresh failure -> re-login
            raise LoginRequired(
                f"Failed to refresh Google credentials for '{account.email}' "
                f"({exc}). Re-authenticate; if the OAuth app is in Testing, "
                "refresh tokens expire after 7 days -- publish to Production."
            ) from exc
        store.set(_cred_key(account.email), creds.to_json())

    if not creds.valid:
        raise LoginRequired(
            f"Google credentials for '{account.email}' are invalid; re-authenticate."
        )
    return creds


def _set_session(session_id: str, status: str, detail: Optional[str]) -> None:
    with _sessions_lock:
        session = _sessions.get(session_id)
        if session is not None:
            session["status"] = status
            session["detail"] = detail


def start_login(account: AccountLike, headless: bool) -> LoginSession:
    """Begin an interactive login.

    Non-headless: runs :meth:`InstalledAppFlow.run_local_server` in a daemon
    thread (opens a loopback listener); poll :func:`poll_login`.
    Headless: returns a ``LoginSession`` with ``auth_url`` for the user to open;
    the caller then passes the pasted redirect URL to :func:`finish_url_login`.
    """
    if headless:
        return _start_headless(account)
    return _start_local_server(account)


def _start_local_server(account: AccountLike) -> LoginSession:
    flow = InstalledAppFlow.from_client_config(_client_config(account), SCOPES)
    session_id = uuid.uuid4().hex
    with _sessions_lock:
        _sessions[session_id] = {"status": "pending", "detail": None, "email": account.email}

    def _worker() -> None:
        try:
            creds = flow.run_local_server(port=0)
            _get_store().set(_cred_key(account.email), creds.to_json())
            _set_session(session_id, "done", None)
        except Exception as exc:  # noqa: BLE001 - surface any failure to the poller
            log.exception("Google local-server login failed for %s", account.email)
            _set_session(session_id, "error", str(exc))

    thread = threading.Thread(target=_worker, name=f"google-login-{session_id}", daemon=True)
    thread.start()
    return LoginSession(session_id=session_id, auth_url=None)


def _start_headless(account: AccountLike) -> LoginSession:
    flow = Flow.from_client_config(
        _client_config(account),
        scopes=SCOPES,
        redirect_uri=_HEADLESS_REDIRECT_URI,
    )
    auth_url, _ = flow.authorization_url(access_type="offline", prompt="consent")
    session_id = uuid.uuid4().hex
    with _sessions_lock:
        _sessions[session_id] = {
            "status": "pending",
            "detail": None,
            "email": account.email,
            "flow": flow,
        }
    return LoginSession(session_id=session_id, auth_url=auth_url)


def finish_url_login(session_id: str, redirect_url: str) -> str:
    """Complete the headless flow from the pasted redirect URL.

    Extracts the authorization code from ``redirect_url``, exchanges it, and
    persists the credentials. Returns ``"done"`` on success.
    """
    with _sessions_lock:
        session = _sessions.get(session_id)
        flow = session.get("flow") if session else None
    if session is None or flow is None:
        raise LoginRequired("Unknown or invalid headless login session.")

    try:
        flow.fetch_token(authorization_response=redirect_url)
    except Exception as exc:  # noqa: BLE001
        _set_session(session_id, "error", str(exc))
        raise LoginRequired(f"Failed to exchange authorization code: {exc}") from exc

    creds = flow.credentials
    _get_store().set(_cred_key(session["email"]), creds.to_json())
    _set_session(session_id, "done", None)
    return "done"


def poll_login(session_id: str) -> str:
    """Return ``"pending"``, ``"done"`` or ``"error: <detail>"`` for a session."""
    with _sessions_lock:
        session = _sessions.get(session_id)
    if session is None:
        return "error: unknown session"
    if session["status"] == "error":
        return f"error: {session['detail']}"
    return session["status"]


def delete_credentials(account: AccountLike) -> None:
    """Remove any stored Google credentials for ``account``."""
    _get_store().delete(_cred_key(account.email))
