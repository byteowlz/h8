"""Google People API contacts backend.

Implements :class:`~h8.providers.base.ContactsBackend` against the People API
v1 (``self._client.people()``), mirroring the response shapes produced by the
EWS implementation in ``h8/contacts.py`` so that existing callers (routes,
Rust client, CLI) see no difference between providers.

Mapping notes:
- ``id`` carries the People API ``resourceName`` (``people/c123...``).
- ``changekey`` carries the People API ``etag``, which the People API requires
  on every update (optimistic concurrency).
- Only the primary (first) entry of each repeated field (``names``,
  ``emailAddresses``, ``phoneNumbers``, ``organizations``) is surfaced, mirroring
  ``h8.contacts._contact_to_dict``'s "first email/phone wins" behavior.
- ``list_contacts`` search is client-side (the People API's ``connections.list``
  has no query parameter), scanning pages and filtering on display name, given
  name, family name and email -- the same fields ``h8.contacts.list_contacts``
  matches against.
"""

from __future__ import annotations

from typing import Any, Dict, List, Optional

from googleapiclient.errors import HttpError

from h8.providers.base import AccountConfig, BackendAuthError, BackendBusyError, BackendError
from h8.providers.google.client import GoogleClient

#: ``personFields`` requested on every read; keep in sync with ``_person_to_dict``.
PERSON_FIELDS = "names,emailAddresses,phoneNumbers,organizations"

#: Maps an ``updates``/``contact_data`` key to the top-level People API field it
#: belongs to, used to build ``updatePersonFields`` from only the changed keys.
_FIELD_TO_PERSON_FIELD = {
    "display_name": "names",
    "given_name": "names",
    "surname": "names",
    "email": "emailAddresses",
    "phone": "phoneNumbers",
    "company": "organizations",
    "job_title": "organizations",
}


def _translate_http_error(exc: HttpError) -> BackendError:
    """Map a googleapiclient ``HttpError`` onto the h8 backend exception hierarchy.

    401/403 -> :class:`BackendAuthError` (the service layer refreshes and
    retries once). 429/5xx -> :class:`BackendBusyError` (backoff and retry).
    Anything else -> :class:`BackendError`.
    """
    status = getattr(getattr(exc, "resp", None), "status", None)
    if status in (401, 403):
        return BackendAuthError(str(exc))
    if status == 429 or (status is not None and 500 <= status < 600):
        return BackendBusyError(str(exc))
    return BackendError(str(exc))


def _execute(request: Any) -> Any:
    """Execute a googleapiclient request, translating HTTP errors (see
    :func:`_translate_http_error`)."""
    try:
        return request.execute()
    except HttpError as exc:
        raise _translate_http_error(exc) from exc


def _execute_or_404(request: Any) -> Optional[Any]:
    """Like :func:`_execute`, but returns ``None`` instead of raising on HTTP 404."""
    try:
        return request.execute()
    except HttpError as exc:
        status = getattr(getattr(exc, "resp", None), "status", None)
        if status == 404:
            return None
        raise _translate_http_error(exc) from exc


def _normalize_resource_name(item_id: str) -> str:
    """Accept a bare People API id or a full ``people/...`` resourceName."""
    return item_id if item_id.startswith("people/") else f"people/{item_id}"


def _matches_search(person: Dict[str, Any], search: str) -> bool:
    """Client-side search filter mirroring ``h8.contacts.list_contacts``.

    Matches on display name, given name, family name, or any email address.
    """
    search_lower = search.lower()
    for name in person.get("names") or []:
        for key in ("displayName", "givenName", "familyName"):
            value = name.get(key)
            if value and search_lower in value.lower():
                return True
    for addr in person.get("emailAddresses") or []:
        value = addr.get("value")
        if value and search_lower in value.lower():
            return True
    return False


def _person_to_dict(person: Dict[str, Any]) -> Dict[str, Any]:
    """Convert a People API ``Person`` resource to the ``h8.contacts`` dict shape."""
    names = person.get("names") or []
    primary_name = names[0] if names else {}

    emails = person.get("emailAddresses") or []
    email = emails[0].get("value") if emails else None

    phones = person.get("phoneNumbers") or []
    phone = phones[0].get("value") if phones else None

    orgs = person.get("organizations") or []
    primary_org = orgs[0] if orgs else {}

    return {
        "id": person.get("resourceName"),
        "changekey": person.get("etag"),
        "display_name": primary_name.get("displayName"),
        "given_name": primary_name.get("givenName"),
        "surname": primary_name.get("familyName"),
        "email": email,
        "phone": phone,
        "company": primary_org.get("name"),
        "job_title": primary_org.get("title"),
    }


def _name_field(display_name: str, given_name: str, surname: str) -> Dict[str, str]:
    """Build a People API ``name`` entry, omitting empty parts."""
    name: Dict[str, str] = {}
    if display_name:
        name["displayName"] = display_name
    if given_name:
        name["givenName"] = given_name
    if surname:
        name["familyName"] = surname
    return name


class GoogleContactsMixin:
    """People API implementation of the ``ContactsBackend`` contract.

    Expects to be mixed into a class providing ``self._client``
    (:class:`~h8.providers.google.client.GoogleClient`) and ``self.account``
    (:class:`~h8.providers.base.AccountConfig`), as :class:`~h8.providers.google.GoogleBackend`
    does.
    """

    _client: GoogleClient
    account: AccountConfig

    def _people_resource(self) -> Any:
        """The ``people`` resource of the People API v1 service."""
        return self._client.people().people()

    # -- ContactsBackend ------------------------------------------------

    def list_contacts(self, limit: int = 100, search: Optional[str] = None) -> List[dict]:
        """List/search the authenticated user's contacts.

        Mirrors ``h8.contacts.list_contacts``: paginates ``connections.list``
        (``resourceName="people/me"``) and, when ``search`` is given, filters
        client-side (the People API has no server-side query for connections),
        stopping once ``limit`` matches have been collected.
        """
        if limit <= 0:
            return []

        resource = self._people_resource()
        results: List[dict] = []
        page_token: Optional[str] = None
        # When searching we may need to scan more than `limit` connections to
        # find `limit` matches, so page in reasonably large chunks.
        page_size = min(limit, 1000) if not search else 200

        while True:
            response = _execute(
                resource.connections().list(
                    resourceName="people/me",
                    personFields=PERSON_FIELDS,
                    pageSize=page_size,
                    pageToken=page_token,
                )
            )
            for person in response.get("connections") or []:
                if search and not _matches_search(person, search):
                    continue
                results.append(_person_to_dict(person))
                if len(results) >= limit:
                    return results
            page_token = response.get("nextPageToken")
            if not page_token:
                break

        return results

    def get_contact(self, item_id: str) -> dict:
        """Fetch one contact by resourceName (or bare People API id).

        Mirrors ``h8.contacts.get_contact``: a missing contact returns
        ``{"error": "Contact not found"}`` rather than raising.
        """
        person = _execute_or_404(
            self._people_resource().get(
                resourceName=_normalize_resource_name(item_id),
                personFields=PERSON_FIELDS,
            )
        )
        if person is None:
            return {"error": "Contact not found"}
        return _person_to_dict(person)

    def create_contact(self, contact_data: dict) -> dict:
        """Create a contact via ``people.createContact``.

        Mirrors ``h8.contacts.create_contact``'s return shape: ``{"id",
        "changekey", "name", "email"}`` (not the full contact dict -- this is
        the existing EWS-derived shape for this one endpoint).
        """
        name = contact_data.get("name", "")
        name_parts = name.split(" ", 1) if name else ["", ""]
        given_name = contact_data.get("given_name") or (name_parts[0] if name_parts else "")
        surname = contact_data.get("surname") or (
            name_parts[1] if len(name_parts) > 1 else ""
        )
        display_name = contact_data.get("display_name") or name

        body: Dict[str, Any] = {}
        name_field = _name_field(display_name, given_name, surname)
        if name_field:
            body["names"] = [name_field]
        if contact_data.get("email"):
            body["emailAddresses"] = [{"value": contact_data["email"]}]
        if contact_data.get("phone"):
            body["phoneNumbers"] = [{"value": contact_data["phone"]}]
        if contact_data.get("company") or contact_data.get("job_title"):
            body["organizations"] = [
                {
                    "name": contact_data.get("company") or "",
                    "title": contact_data.get("job_title") or "",
                }
            ]

        created = _execute(self._people_resource().createContact(body=body))

        return {
            "id": created.get("resourceName"),
            "changekey": created.get("etag"),
            "name": display_name,
            "email": contact_data.get("email"),
        }

    def delete_contact(self, item_id: str) -> dict:
        """Delete a contact via ``people.deleteContact``.

        Mirrors ``h8.contacts.delete_contact``'s ``{"success", ...}`` shape.
        Auth/busy errors still propagate so the service layer can refresh and
        retry; only "not found" and other backend errors are swallowed into a
        ``success: False`` result.
        """
        resource_name = _normalize_resource_name(item_id)
        try:
            result = _execute_or_404(
                self._people_resource().deleteContact(resourceName=resource_name)
            )
        except (BackendAuthError, BackendBusyError):
            raise
        except BackendError as exc:
            return {"success": False, "error": f"Failed to delete contact: {exc}"}
        if result is None:
            return {"success": False, "error": "Contact not found"}
        return {"success": True, "id": item_id}

    def update_contact(self, item_id: str, updates: dict) -> dict:
        """Update a contact via ``people.updateContact``.

        Builds ``updatePersonFields`` from only the keys present in ``updates``
        (mirrors ``h8.contacts.update_contact``'s partial-update semantics).
        The People API requires the current ``etag`` on every update; if the
        caller did not pass one through ``updates["changekey"]`` (the current
        ``/contacts/{id}`` route never does), the contact is re-fetched to get
        it. Partial name/organization updates are merged onto the current
        values so an update to only ``display_name`` (say) does not clobber
        ``given_name``/``surname`` -- the People API replaces the whole
        ``names``/``organizations`` list for any field named in
        ``updatePersonFields``.
        """
        resource_name = _normalize_resource_name(item_id)
        changed_fields = [key for key in updates if key in _FIELD_TO_PERSON_FIELD]
        if not changed_fields:
            return self.get_contact(item_id)

        person_fields_touched = sorted(
            {_FIELD_TO_PERSON_FIELD[key] for key in changed_fields}
        )

        etag = updates.get("changekey")
        current: Optional[Dict[str, Any]] = None

        def _current() -> Dict[str, Any]:
            nonlocal current
            if current is None:
                current = _execute(
                    self._people_resource().get(
                        resourceName=resource_name, personFields=PERSON_FIELDS
                    )
                )
            return current

        if not etag:
            etag = _current().get("etag")

        body: Dict[str, Any] = {"etag": etag}

        if "names" in person_fields_touched:
            names = _current().get("names") or [{}]
            primary = dict(names[0])
            if "display_name" in updates:
                primary["displayName"] = updates["display_name"]
            if "given_name" in updates:
                primary["givenName"] = updates["given_name"]
            if "surname" in updates:
                primary["familyName"] = updates["surname"]
            body["names"] = [primary]

        if "emailAddresses" in person_fields_touched:
            body["emailAddresses"] = [{"value": updates["email"]}]

        if "phoneNumbers" in person_fields_touched:
            body["phoneNumbers"] = [{"value": updates["phone"]}]

        if "organizations" in person_fields_touched:
            orgs = _current().get("organizations") or [{}]
            primary_org = dict(orgs[0])
            if "company" in updates:
                primary_org["name"] = updates["company"]
            if "job_title" in updates:
                primary_org["title"] = updates["job_title"]
            body["organizations"] = [primary_org]

        updated = _execute(
            self._people_resource().updateContact(
                resourceName=resource_name,
                updatePersonFields=",".join(person_fields_touched),
                body=body,
            )
        )
        return _person_to_dict(updated)
