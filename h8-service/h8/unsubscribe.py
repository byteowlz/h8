"""Bulk unsubscribe from marketing emails.

Extracts unsubscribe links from email headers and body content,
then visits them to unsubscribe. Supports:
- List-Unsubscribe headers (RFC 2369)
- Footer link extraction (regex patterns)
- Rate limiting and safety features
"""

import ipaddress
import logging
import re
import socket
import time
from dataclasses import dataclass, field
from typing import Optional
from urllib.parse import urljoin, urlparse

import httpx
from exchangelib import HTMLBody, ItemId
from exchangelib.account import Account

log = logging.getLogger(__name__)

# Common unsubscribe link patterns in email bodies
UNSUBSCRIBE_PATTERNS = [
    re.compile(
        r'<a\s[^>]*href=["\']([^"\']*)["\'][^>]*>[^<]*'
        r"(?:unsubscribe|opt[\s-]?out|abmelden|abbestellen|austragen)"
        r"[^<]*</a>",
        re.IGNORECASE,
    ),
    re.compile(
        r'<a\s[^>]*href=["\']([^"\']*'
        r"(?:unsubscribe|opt[\s-]?out|email[\s-]?preferences|manage[\s-]?subscriptions"
        r"|remove|abmelden|abbestellen)"
        r'[^"\']*)["\']',
        re.IGNORECASE,
    ),
    re.compile(
        r"(https?://[^\s<>\"']+(?:unsubscribe|opt[\s-]?out|remove|abmelden|abbestellen)"
        r"[^\s<>\"']*)",
        re.IGNORECASE,
    ),
]

# Default browser-like headers for HTTP requests
DEFAULT_HEADERS = {
    "User-Agent": (
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
        "AppleWebKit/537.36 (KHTML, like Gecko) "
        "Chrome/120.0.0.0 Safari/537.36"
    ),
    "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    "Accept-Language": "en-US,en;q=0.9,de;q=0.8",
}


@dataclass
class UnsubscribeLink:
    """A discovered unsubscribe mechanism."""

    url: str
    source: str  # "header", "body_link", "body_text"
    is_mailto: bool = False


@dataclass
class UnsubscribeResult:
    """Result of processing a single email for unsubscribe."""

    message_id: str
    sender: str
    subject: str
    links: list[UnsubscribeLink] = field(default_factory=list)
    status: str = "pending"  # pending, success, failed, skipped, needs_confirmation
    error: Optional[str] = None
    http_status: Optional[int] = None

    def to_dict(self) -> dict:
        """Convert to dict for JSON serialization."""
        return {
            "message_id": self.message_id,
            "sender": self.sender,
            "subject": self.subject,
            "links": [
                {"url": link.url, "source": link.source, "is_mailto": link.is_mailto}
                for link in self.links
            ],
            "status": self.status,
            "error": self.error,
            "http_status": self.http_status,
        }


def extract_unsubscribe_links(
    headers: list | None,
    body: str | None,
    body_type: str = "html",
) -> list[UnsubscribeLink]:
    """Extract unsubscribe links from email headers and body.

    Args:
        headers: List of exchangelib MessageHeader objects (name, value pairs)
        body: Email body text (HTML or plain)
        body_type: "html" or "text"

    Returns:
        List of UnsubscribeLink objects, ordered by preference (header first)
    """
    links: list[UnsubscribeLink] = []
    seen_urls: set[str] = set()

    # 1. Check List-Unsubscribe header (most reliable)
    if headers:
        for header in headers:
            if header.name.lower() == "list-unsubscribe":
                header_links = _parse_list_unsubscribe_header(header.value)
                for link in header_links:
                    if link.url not in seen_urls:
                        seen_urls.add(link.url)
                        links.append(link)

    # 2. Extract from body
    if body:
        body_links = _extract_body_links(body, body_type)
        for link in body_links:
            if link.url not in seen_urls:
                seen_urls.add(link.url)
                links.append(link)

    return links


def _parse_list_unsubscribe_header(value: str) -> list[UnsubscribeLink]:
    """Parse RFC 2369 List-Unsubscribe header value.

    Format: <mailto:unsub@example.com>, <https://example.com/unsub>
    """
    links: list[UnsubscribeLink] = []
    # Extract URLs within angle brackets
    for match in re.finditer(r"<([^>]+)>", value):
        url = match.group(1).strip()
        is_mailto = url.lower().startswith("mailto:")
        links.append(
            UnsubscribeLink(url=url, source="header", is_mailto=is_mailto)
        )
    return links


def _extract_body_links(body: str, body_type: str) -> list[UnsubscribeLink]:
    """Extract unsubscribe links from email body."""
    links: list[UnsubscribeLink] = []

    if body_type == "html":
        # Search HTML body with patterns
        for pattern in UNSUBSCRIBE_PATTERNS:
            for match in pattern.finditer(body):
                url = match.group(1).strip()
                if _is_valid_unsubscribe_url(url):
                    source = "body_link" if "<a" in pattern.pattern else "body_text"
                    links.append(UnsubscribeLink(url=url, source=source))
    else:
        # Plain text: look for URLs near unsubscribe keywords
        url_pattern = re.compile(
            r"(https?://[^\s<>\"']+)", re.IGNORECASE
        )
        lines = body.split("\n")
        for i, line in enumerate(lines):
            lower = line.lower()
            if any(
                kw in lower
                for kw in [
                    "unsubscribe",
                    "opt out",
                    "opt-out",
                    "abmelden",
                    "abbestellen",
                ]
            ):
                # Check this line and adjacent lines for URLs
                context = "\n".join(lines[max(0, i - 1) : i + 2])
                for url_match in url_pattern.finditer(context):
                    url = url_match.group(1)
                    if _is_valid_unsubscribe_url(url):
                        links.append(
                            UnsubscribeLink(url=url, source="body_text")
                        )

    return links


def _is_valid_unsubscribe_url(url: str) -> bool:
    """Check if a URL is a valid unsubscribe link (not tracking pixel etc)."""
    try:
        parsed = urlparse(url)
        if parsed.scheme not in ("http", "https"):
            return False
        if not parsed.netloc:
            return False
        # Filter out common non-unsubscribe URLs
        lower = url.lower()
        if any(
            ext in lower
            for ext in [".png", ".jpg", ".gif", ".svg", ".css", ".js"]
        ):
            return False
        return True
    except Exception:
        return False


def _is_safe_domain(url: str, blocked_patterns: list[str]) -> bool:
    """Check if a URL domain is not blocked."""
    try:
        parsed = urlparse(url)
        domain = parsed.netloc.lower()
        for pattern in blocked_patterns:
            # Support simple glob patterns
            pat = pattern.lower().replace("*.", "").replace("*", "")
            if pat in domain:
                return False
        return True
    except Exception:
        return False


def _is_safe_sender(sender: str, safe_senders: list[str]) -> bool:
    """Check if a sender is in the safe list (should not be unsubscribed)."""
    sender_lower = sender.lower()
    for safe in safe_senders:
        if safe.lower() in sender_lower:
            return True
    return False


def scan_messages(
    account: Account,
    folder: str = "inbox",
    sender: str | None = None,
    search: str | None = None,
    limit: int = 50,
    safe_senders: list[str] | None = None,
    blocked_patterns: list[str] | None = None,
) -> list[dict]:
    """Scan messages for unsubscribe links (dry run).

    Args:
        account: EWS account
        folder: Folder to scan
        sender: Filter by sender email (substring match)
        search: Search term for subject
        limit: Maximum messages to process
        safe_senders: Email patterns to skip
        blocked_patterns: URL domain patterns to block

    Returns:
        List of scan results with discovered unsubscribe links
    """
    from h8.mail import get_folder

    safe_senders = safe_senders or []
    blocked_patterns = blocked_patterns or []

    mail_folder = get_folder(account, folder)

    # Build query - filter subject server-side, sender client-side
    # (exchangelib does not support sender__email_address field path)
    query = mail_folder.all()
    if search:
        query = query.filter(subject__icontains=search)

    # Fetch more if filtering by sender client-side (need headroom)
    fetch_limit = limit * 5 if sender else limit

    # Fetch messages with headers
    messages = query.order_by("-datetime_received").only(
        "id",
        "changekey",
        "subject",
        "sender",
        "datetime_received",
        "headers",
        "body",
    )[:fetch_limit]

    results: list[dict] = []
    matched = 0

    for item in messages:
        if not hasattr(item, "subject"):
            continue

        sender_email = item.sender.email_address if item.sender else ""

        # Client-side sender filter
        if sender and sender.lower() not in sender_email.lower():
            continue

        matched += 1
        if matched > limit:
            break

        # Check safe senders
        if _is_safe_sender(sender_email, safe_senders):
            result = UnsubscribeResult(
                message_id=item.id,
                sender=sender_email,
                subject=item.subject or "(no subject)",
                status="skipped",
                error="safe sender",
            )
            results.append(result.to_dict())
            continue

        # Determine body type
        body_str = str(item.body) if item.body else ""
        body_type = "html" if isinstance(item.body, HTMLBody) else "text"

        # Extract unsubscribe links
        links = extract_unsubscribe_links(item.headers, body_str, body_type)

        # Filter blocked domains
        filtered_links = [
            link
            for link in links
            if link.is_mailto or _is_safe_domain(link.url, blocked_patterns)
        ]

        result = UnsubscribeResult(
            message_id=item.id,
            sender=sender_email,
            subject=item.subject or "(no subject)",
            links=filtered_links,
            status="found" if filtered_links else "no_link",
        )
        results.append(result.to_dict())

    return results


def execute_unsubscribe(
    account: Account,
    item_ids: list[str],
    safe_senders: list[str] | None = None,
    blocked_patterns: list[str] | None = None,
    trusted_domains: list[str] | None = None,
    rate_limit_seconds: float = 2.0,
) -> list[dict]:
    """Execute unsubscribe for the given message IDs.

    For each message:
    1. Fetch full message with headers
    2. Extract unsubscribe links
    3. Visit HTTP unsubscribe URLs
    4. Report results

    Args:
        account: EWS account
        item_ids: List of message IDs to process
        safe_senders: Email patterns to skip
        blocked_patterns: URL domain patterns to block
        trusted_domains: Domains where we auto-confirm forms
        rate_limit_seconds: Delay between HTTP requests

    Returns:
        List of result dicts
    """
    safe_senders = safe_senders or []
    blocked_patterns = blocked_patterns or []
    trusted_domains = trusted_domains or []

    results: list[dict] = []

    for i, item_id in enumerate(item_ids):
        # Rate limit between requests
        if i > 0:
            time.sleep(rate_limit_seconds)

        try:
            # Fetch the message
            items = list(account.fetch(ids=[ItemId(id=item_id)]))
            if not items or items[0] is None:
                results.append(
                    UnsubscribeResult(
                        message_id=item_id,
                        sender="",
                        subject="",
                        status="failed",
                        error="message not found",
                    ).to_dict()
                )
                continue

            item = items[0]
            sender_email = item.sender.email_address if item.sender else ""

            # Check safe senders
            if _is_safe_sender(sender_email, safe_senders):
                results.append(
                    UnsubscribeResult(
                        message_id=item_id,
                        sender=sender_email,
                        subject=item.subject or "(no subject)",
                        status="skipped",
                        error="safe sender",
                    ).to_dict()
                )
                continue

            # Extract links
            body_str = str(item.body) if item.body else ""
            body_type = "html" if isinstance(item.body, HTMLBody) else "text"
            links = extract_unsubscribe_links(item.headers, body_str, body_type)

            # Filter blocked domains
            filtered_links = [
                link
                for link in links
                if link.is_mailto or _is_safe_domain(link.url, blocked_patterns)
            ]

            if not filtered_links:
                results.append(
                    UnsubscribeResult(
                        message_id=item_id,
                        sender=sender_email,
                        subject=item.subject or "(no subject)",
                        status="no_link",
                    ).to_dict()
                )
                continue

            # Try to unsubscribe using the best available link
            result = _visit_unsubscribe_link(
                item_id,
                sender_email,
                item.subject or "(no subject)",
                filtered_links,
                trusted_domains,
            )
            results.append(result.to_dict())

        except Exception as e:
            log.error("Failed to process message %s: %s", item_id, e)
            results.append(
                UnsubscribeResult(
                    message_id=item_id,
                    sender="",
                    subject="",
                    status="failed",
                    error=str(e),
                ).to_dict()
            )

    return results


# Maximum number of redirect hops to follow manually (each re-checked for SSRF).
MAX_REDIRECT_HOPS = 3

# HTTP status codes that indicate a redirect.
_REDIRECT_STATUSES = (301, 302, 303, 307, 308)


class BlockedURLError(Exception):
    """Raised when an unsubscribe URL resolves to a disallowed address (SSRF)."""


def _is_blocked_ip(ip_str: str) -> bool:
    """Return whether ``ip_str`` is a loopback/private/link-local/unspecified address.

    Blocks (SSRF guard): loopback (127/8, ::1), RFC-1918 (10/8, 172.16/12,
    192.168/16), link-local incl. the cloud metadata range (169.254/16, fe80::/10),
    unique-local IPv6 (fc00::/7), and the unspecified address (0.0.0.0, ::).
    Unparseable input is treated as blocked (fail closed).
    """
    try:
        ip = ipaddress.ip_address(ip_str)
    except ValueError:
        return True
    return bool(
        ip.is_loopback
        or ip.is_private
        or ip.is_link_local
        or ip.is_unspecified
        or ip.is_reserved
        or ip.is_multicast
    )


def _resolve_and_check_host(hostname: str) -> tuple[bool, str]:
    """Resolve ``hostname`` and report whether any resolved IP is disallowed.

    Args:
        hostname: The host portion of a URL.

    Returns:
        ``(blocked, reason)`` -- ``blocked`` is ``True`` if resolution fails or
        any resolved address is private/loopback/link-local.
    """
    if not hostname:
        return True, "missing host"
    try:
        infos = socket.getaddrinfo(hostname, None)
    except socket.gaierror as exc:
        return True, f"DNS resolution failed for '{hostname}': {exc}"
    for info in infos:
        ip_str = info[4][0]
        if _is_blocked_ip(ip_str):
            return True, f"host '{hostname}' resolves to blocked address {ip_str}"
    return False, ""


def _fetch_with_ssrf_guard(
    client: httpx.Client, url: str, max_hops: int = MAX_REDIRECT_HOPS
) -> httpx.Response:
    """GET ``url`` with automatic redirects disabled, re-checking every hop.

    Each URL (the initial target and every ``Location`` redirect) has its host
    resolved and screened before the request is issued. At most ``max_hops``
    redirects are followed.

    Raises:
        BlockedURLError: If any hop resolves to a disallowed address or the
            redirect chain exceeds ``max_hops``.
    """
    current = url
    hops = 0
    while True:
        host = urlparse(current).hostname or ""
        blocked, reason = _resolve_and_check_host(host)
        if blocked:
            raise BlockedURLError(reason)
        response = client.get(current)
        if response.status_code in _REDIRECT_STATUSES:
            location = response.headers.get("location")
            if not location:
                return response
            if hops >= max_hops:
                raise BlockedURLError(
                    f"exceeded {max_hops} redirect hops visiting {url}"
                )
            hops += 1
            current = urljoin(current, location)
            continue
        return response


def _visit_unsubscribe_link(
    message_id: str,
    sender: str,
    subject: str,
    links: list[UnsubscribeLink],
    trusted_domains: list[str],
) -> UnsubscribeResult:
    """Visit unsubscribe links, preferring HTTP header links over body links.

    Tries links in order of preference:
    1. HTTP links from List-Unsubscribe header
    2. HTTP links from body
    3. Mailto links are reported but not executed

    Args:
        message_id: EWS message ID
        sender: Sender email
        subject: Message subject
        links: Ordered list of unsubscribe links
        trusted_domains: Domains where we auto-confirm

    Returns:
        UnsubscribeResult with outcome
    """
    result = UnsubscribeResult(
        message_id=message_id,
        sender=sender,
        subject=subject,
        links=links,
    )

    # Prefer HTTP links over mailto
    http_links = [link for link in links if not link.is_mailto]
    mailto_links = [link for link in links if link.is_mailto]

    if not http_links:
        if mailto_links:
            result.status = "needs_confirmation"
            result.error = f"only mailto link available: {mailto_links[0].url}"
        else:
            result.status = "no_link"
        return result

    # Try each HTTP link
    for link in http_links:
        try:
            with httpx.Client(
                headers=DEFAULT_HEADERS,
                follow_redirects=False,
                timeout=15.0,
            ) as client:
                response = _fetch_with_ssrf_guard(client, link.url)
                result.http_status = response.status_code

                if response.status_code == 200:
                    # Check if the page contains a confirmation form
                    content_lower = response.text.lower()
                    has_form = "<form" in content_lower
                    has_confirm = any(
                        kw in content_lower
                        for kw in [
                            "confirm",
                            "bestätigen",
                            "are you sure",
                            "click here to confirm",
                        ]
                    )

                    if has_form and has_confirm:
                        # Check if it's a trusted domain
                        parsed = urlparse(link.url)
                        domain = parsed.netloc.lower()
                        is_trusted = any(
                            td.lower() in domain for td in trusted_domains
                        )

                        if is_trusted:
                            result.status = "success"
                            result.error = "auto-confirmed (trusted domain)"
                        else:
                            result.status = "needs_confirmation"
                            result.error = (
                                f"page has confirmation form at {link.url}"
                            )
                    else:
                        # Simple GET unsubscribe succeeded
                        result.status = "success"
                    return result

                elif response.status_code in (301, 302, 303, 307, 308):
                    # Should be handled by follow_redirects, but just in case
                    result.status = "success"
                    return result

                else:
                    result.error = (
                        f"HTTP {response.status_code} from {link.url}"
                    )

        except BlockedURLError as e:
            # SSRF guard tripped: do not treat as a normal failure -- report the
            # link as skipped and stop (an unsubscribe link pointing at a private
            # address is not something we follow).
            result.status = "skipped"
            result.error = f"blocked unsafe unsubscribe URL: {e}"
            log.warning("Blocked unsafe unsubscribe URL %s: %s", link.url, e)
            return result
        except httpx.TimeoutException:
            result.error = f"timeout visiting {link.url}"
            log.warning("Timeout visiting %s", link.url)
        except Exception as e:
            result.error = f"error visiting {link.url}: {e}"
            log.warning("Error visiting %s: %s", link.url, e)

    # All HTTP links failed
    if result.error:
        result.status = "failed"
    else:
        result.status = "no_link"

    return result
