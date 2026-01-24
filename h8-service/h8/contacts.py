"""Contacts operations."""

from typing import Optional
from exchangelib import Contact
from exchangelib.indexed_properties import EmailAddress, PhoneNumber
from exchangelib.account import Account


def list_contacts(
    account: Account, limit: int = 100, search: Optional[str] = None
) -> list[dict]:
    """List contacts, optionally filtered by search query."""
    contacts = []

    # Use .only() to fetch only required fields - avoids fetching large data
    base_query = account.contacts.all().only(
        "id",
        "changekey",
        "display_name",
        "given_name",
        "surname",
        "email_addresses",
        "phone_numbers",
        "company_name",
        "job_title",
    )

    if search:
        search_lower = search.lower()
        for item in base_query:
            if not isinstance(item, Contact):
                continue

            # Search in display_name, email, given_name, surname
            match = False
            if item.display_name and search_lower in item.display_name.lower():
                match = True
            elif item.given_name and search_lower in item.given_name.lower():
                match = True
            elif item.surname and search_lower in item.surname.lower():
                match = True
            else:
                # Check email addresses
                if item.email_addresses:
                    for addr in item.email_addresses:
                        if addr:
                            email = getattr(addr, "email", None) or getattr(
                                addr, "email_address", None
                            )
                            if email and search_lower in email.lower():
                                match = True
                                break

            if match:
                contacts.append(_contact_to_dict(item))
                if len(contacts) >= limit:
                    break
    else:
        for item in base_query[:limit]:
            if not isinstance(item, Contact):
                continue
            contacts.append(_contact_to_dict(item))

    return contacts


def get_contact(account: Account, item_id: str) -> dict:
    """Get a contact by ID."""
    from exchangelib import ItemId

    # Fetch item by ID using account.fetch() - EWS IDs are globally unique
    try:
        items = list(account.fetch(ids=[ItemId(id=item_id)]))
        if items and items[0] is not None:
            return _contact_to_dict(items[0])
    except Exception:
        pass

    return {"error": "Contact not found"}


def create_contact(account: Account, contact_data: dict) -> dict:
    """Create a contact from JSON data."""
    # Parse name
    name = contact_data.get("name", "")
    name_parts = name.split(" ", 1) if name else ["", ""]
    given_name = name_parts[0] if name_parts else ""
    surname = name_parts[1] if len(name_parts) > 1 else ""

    # Build email addresses
    email_addresses = None
    if contact_data.get("email"):
        email_addresses = [
            EmailAddress(email=contact_data["email"], label="EmailAddress1")
        ]

    # Build phone numbers
    phone_numbers = None
    if contact_data.get("phone"):
        phone_numbers = [
            PhoneNumber(phone_number=contact_data["phone"], label="BusinessPhone")
        ]

    contact = Contact(
        account=account,
        folder=account.contacts,
        given_name=contact_data.get("given_name", given_name),
        surname=contact_data.get("surname", surname),
        display_name=contact_data.get("display_name", name),
        email_addresses=email_addresses,
        phone_numbers=phone_numbers,
        company_name=contact_data.get("company"),
        job_title=contact_data.get("job_title"),
    )

    contact.save()

    return {
        "id": contact.id,
        "changekey": contact.changekey,
        "name": contact.display_name,
        "email": contact_data.get("email"),
    }


def delete_contact(account: Account, item_id: str) -> dict:
    """Delete a contact by ID."""
    from exchangelib import ItemId

    # Fetch item by ID using account.fetch() - EWS IDs are globally unique
    try:
        items = list(account.fetch(ids=[ItemId(id=item_id)]))
        if items and items[0] is not None:
            items[0].delete()
            return {"success": True, "id": item_id}
    except Exception as e:
        return {"success": False, "error": f"Failed to delete contact: {e}"}

    return {"success": False, "error": "Contact not found"}


def update_contact(account: Account, item_id: str, updates: dict) -> dict:
    """Update an existing contact.

    Args:
        account: EWS account
        item_id: Contact ID
        updates: Dict of fields to update. Supported fields:
            - display_name, given_name, surname
            - email (primary email address)
            - phone (primary phone number)
            - company, job_title
    """
    from exchangelib import ItemId

    try:
        items = list(account.fetch(ids=[ItemId(id=item_id)]))
        if not items or items[0] is None:
            return {"success": False, "error": "Contact not found"}

        contact = items[0]

        # Update simple string fields
        if "display_name" in updates:
            contact.display_name = updates["display_name"]
        if "given_name" in updates:
            contact.given_name = updates["given_name"]
        if "surname" in updates:
            contact.surname = updates["surname"]
        if "company" in updates:
            contact.company_name = updates["company"]
        if "job_title" in updates:
            contact.job_title = updates["job_title"]

        # Update email - replace primary email
        if "email" in updates:
            contact.email_addresses = [
                EmailAddress(email=updates["email"], label="EmailAddress1")
            ]

        # Update phone - replace primary phone
        if "phone" in updates:
            contact.phone_numbers = [
                PhoneNumber(phone_number=updates["phone"], label="BusinessPhone")
            ]

        contact.save(update_fields=list(updates.keys()))

        return _contact_to_dict(contact)
    except Exception as e:
        return {"success": False, "error": f"Failed to update contact: {e}"}


def _contact_to_dict(contact: Contact) -> dict:
    """Convert a Contact to a dictionary."""
    # Get primary email
    email = None
    if contact.email_addresses:
        for addr in contact.email_addresses:
            if addr:
                # EmailAddress object has email attribute
                if hasattr(addr, "email"):
                    email = addr.email
                elif hasattr(addr, "email_address"):
                    email = addr.email_address
                else:
                    email = str(addr)
                break

    # Get primary phone
    phone = None
    if contact.phone_numbers:
        for p in contact.phone_numbers:
            if p:
                if hasattr(p, "phone_number"):
                    phone = p.phone_number
                else:
                    phone = str(p)
                break

    return {
        "id": contact.id,
        "changekey": contact.changekey,
        "display_name": contact.display_name,
        "given_name": contact.given_name,
        "surname": contact.surname,
        "email": email,
        "phone": phone,
        "company": contact.company_name,
        "job_title": contact.job_title,
    }
