"""
Inbox rules and Out-of-Office (OOF) automation for Exchange.

This module provides CRUD operations for inbox rules and OOF settings
via the EWS ManageInboxRules and GetUserOofSettings/SetUserOofSettings APIs.
"""

from typing import Any, Dict, List, Optional

from exchangelib import Account
from exchangelib.properties import Rule, Conditions, Actions
from exchangelib.settings import OofSettings


def _rule_to_dict(rule: Rule) -> Dict[str, Any]:
    """Convert a Rule object to a serializable dictionary."""
    result = {
        "id": rule.id,
        "display_name": rule.display_name,
        "priority": rule.priority,
        "is_enabled": rule.is_enabled,
        "is_not_supported": rule.is_not_supported,
        "is_in_error": rule.is_in_error,
    }

    # Convert conditions
    if rule.conditions:
        conditions = {}
        for field in rule.conditions.FIELDS:
            value = getattr(rule.conditions, field.name)
            if value is not None:
                if hasattr(value, "__iter__") and not isinstance(value, str):
                    # Handle lists of email addresses or strings
                    items = []
                    for item in value:
                        if hasattr(item, "email_address"):
                            items.append(item.email_address)
                        else:
                            items.append(str(item))
                    if items:
                        conditions[field.name] = items
                else:
                    conditions[field.name] = value
        if conditions:
            result["conditions"] = conditions

    # Convert actions
    if rule.actions:
        actions = {}
        for field in rule.actions.FIELDS:
            value = getattr(rule.actions, field.name)
            if value is not None:
                if field.name in ("move_to_folder", "copy_to_folder") and value:
                    actions[field.name] = str(value.folder_id) if hasattr(value, "folder_id") else str(value)
                elif hasattr(value, "__iter__") and not isinstance(value, str):
                    items = []
                    for item in value:
                        if hasattr(item, "email_address"):
                            items.append(item.email_address)
                        else:
                            items.append(str(item))
                    if items:
                        actions[field.name] = items
                else:
                    actions[field.name] = value
        if actions:
            result["actions"] = actions

    return result


def _dict_to_conditions(data: Dict[str, Any]) -> Optional[Conditions]:
    """Convert a dictionary to a Conditions object."""
    if not data:
        return None

    kwargs = {}
    for key, value in data.items():
        if value is not None:
            kwargs[key] = value

    return Conditions(**kwargs) if kwargs else None


def _dict_to_actions(data: Dict[str, Any]) -> Optional[Actions]:
    """Convert a dictionary to an Actions object."""
    if not data:
        return None

    kwargs = {}
    for key, value in data.items():
        if value is not None:
            kwargs[key] = value

    return Actions(**kwargs) if kwargs else None


def list_rules(account: Account) -> List[Dict[str, Any]]:
    """List all inbox rules for the account.

    Args:
        account: The Exchange account to query.

    Returns:
        List of rule dictionaries.
    """
    rules = account.rules
    return [_rule_to_dict(rule) for rule in rules]


def get_rule(account: Account, rule_id: str) -> Optional[Dict[str, Any]]:
    """Get a specific rule by ID.

    Args:
        account: The Exchange account to query.
        rule_id: The rule ID to find.

    Returns:
        Rule dictionary or None if not found.
    """
    for rule in account.rules:
        if rule.id == rule_id:
            return _rule_to_dict(rule)
    return None


def create_rule(
    account: Account,
    display_name: str,
    priority: int = 1,
    is_enabled: bool = True,
    conditions: Optional[Dict[str, Any]] = None,
    actions: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    """Create a new inbox rule.

    Args:
        account: The Exchange account.
        display_name: Human-readable name for the rule.
        priority: Execution priority (1 = highest).
        is_enabled: Whether the rule is active.
        conditions: Dict of conditions (from_addresses, contains_subject_strings, etc.).
        actions: Dict of actions (move_to_folder, delete, mark_as_read, etc.).

    Returns:
        Created rule dictionary with assigned ID.
    """
    rule = Rule(
        account=account,
        display_name=display_name,
        priority=priority,
        is_enabled=is_enabled,
        conditions=_dict_to_conditions(conditions),
        actions=_dict_to_actions(actions) or Actions(),
    )

    account.create_rule(rule)
    return _rule_to_dict(rule)


def update_rule(
    account: Account,
    rule_id: str,
    display_name: Optional[str] = None,
    priority: Optional[int] = None,
    is_enabled: Optional[bool] = None,
    conditions: Optional[Dict[str, Any]] = None,
    actions: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    """Update an existing inbox rule.

    Args:
        account: The Exchange account.
        rule_id: The ID of the rule to update.
        display_name: New display name (optional).
        priority: New priority (optional).
        is_enabled: New enabled state (optional).
        conditions: New conditions (optional, replaces existing).
        actions: New actions (optional, replaces existing).

    Returns:
        Updated rule dictionary.

    Raises:
        ValueError: If rule is not found.
    """
    # Find the existing rule
    existing_rule = None
    for rule in account.rules:
        if rule.id == rule_id:
            existing_rule = rule
            break

    if not existing_rule:
        raise ValueError(f"Rule with ID '{rule_id}' not found")

    # Update fields
    if display_name is not None:
        existing_rule.display_name = display_name
    if priority is not None:
        existing_rule.priority = priority
    if is_enabled is not None:
        existing_rule.is_enabled = is_enabled
    if conditions is not None:
        existing_rule.conditions = _dict_to_conditions(conditions)
    if actions is not None:
        existing_rule.actions = _dict_to_actions(actions)

    account.set_rule(existing_rule)
    return _rule_to_dict(existing_rule)


def enable_rule(account: Account, rule_id: str) -> Dict[str, Any]:
    """Enable an inbox rule.

    Args:
        account: The Exchange account.
        rule_id: The ID of the rule to enable.

    Returns:
        Updated rule dictionary.
    """
    return update_rule(account, rule_id, is_enabled=True)


def disable_rule(account: Account, rule_id: str) -> Dict[str, Any]:
    """Disable an inbox rule.

    Args:
        account: The Exchange account.
        rule_id: The ID of the rule to disable.

    Returns:
        Updated rule dictionary.
    """
    return update_rule(account, rule_id, is_enabled=False)


def delete_rule(account: Account, rule_id: str) -> None:
    """Delete an inbox rule.

    Args:
        account: The Exchange account.
        rule_id: The ID of the rule to delete.

    Raises:
        ValueError: If rule is not found.
    """
    # Find the existing rule
    existing_rule = None
    for rule in account.rules:
        if rule.id == rule_id:
            existing_rule = rule
            break

    if not existing_rule:
        raise ValueError(f"Rule with ID '{rule_id}' not found")

    account.delete_rule(existing_rule)


def get_oof_settings(account: Account) -> Dict[str, Any]:
    """Get Out-of-Office settings.

    Args:
        account: The Exchange account.

    Returns:
        Dictionary with OOF settings.
    """
    settings = account.oof_settings

    result = {
        "state": settings.state,
        "external_audience": settings.external_audience,
    }

    if settings.start:
        result["start"] = settings.start.isoformat()
    if settings.end:
        result["end"] = settings.end.isoformat()
    if settings.internal_reply:
        result["internal_reply"] = settings.internal_reply
    if settings.external_reply:
        result["external_reply"] = settings.external_reply

    # Derived properties for convenience
    result["enabled"] = settings.state in (OofSettings.ENABLED, OofSettings.SCHEDULED)
    result["scheduled"] = settings.state == OofSettings.SCHEDULED

    return result


def set_oof_settings(
    account: Account,
    state: str,
    external_audience: Optional[str] = None,
    start: Optional[str] = None,
    end: Optional[str] = None,
    internal_reply: Optional[str] = None,
    external_reply: Optional[str] = None,
) -> Dict[str, Any]:
    """Set Out-of-Office settings.

    Args:
        account: The Exchange account.
        state: One of 'Enabled', 'Scheduled', or 'Disabled'.
        external_audience: One of 'All', 'Known', or 'None'.
        start: ISO datetime string for scheduled start (required if state='Scheduled').
        end: ISO datetime string for scheduled end (required if state='Scheduled').
        internal_reply: Auto-reply message for internal senders.
        external_reply: Auto-reply message for external senders.

    Returns:
        Updated OOF settings dictionary.
    """
    import datetime
    from datetime import timezone

    # Parse datetimes if provided
    start_dt = None
    end_dt = None

    if start:
        # Try parsing ISO format
        try:
            start_dt = datetime.datetime.fromisoformat(start.replace("Z", "+00:00"))
        except ValueError:
            # Try parsing with common formats
            for fmt in ("%Y-%m-%d %H:%M", "%Y-%m-%d"):
                try:
                    start_dt = datetime.datetime.strptime(start, fmt)
                    start_dt = start_dt.replace(tzinfo=datetime.timezone.utc)
                    break
                except ValueError:
                    continue

    if end:
        try:
            end_dt = datetime.datetime.fromisoformat(end.replace("Z", "+00:00"))
        except ValueError:
            for fmt in ("%Y-%m-%d %H:%M", "%Y-%m-%d"):
                try:
                    end_dt = datetime.datetime.strptime(end, fmt)
                    end_dt = end_dt.replace(tzinfo=datetime.timezone.utc)
                    break
                except ValueError:
                    continue

    # Ensure UTC timezone
    if start_dt and start_dt.tzinfo is None:
        start_dt = start_dt.replace(tzinfo=timezone.utc)
    if end_dt and end_dt.tzinfo is None:
        end_dt = end_dt.replace(tzinfo=timezone.utc)

    settings = OofSettings(
        state=state,
        external_audience=external_audience or "All",
        start=start_dt,
        end=end_dt,
        internal_reply=internal_reply or "",
        external_reply=external_reply or "",
    )

    account.oof_settings = settings
    return get_oof_settings(account)


def enable_oof(
    account: Account,
    internal_reply: str,
    external_reply: Optional[str] = None,
    external_audience: str = "All",
) -> Dict[str, Any]:
    """Enable Out-of-Office (immediate, not scheduled).

    Args:
        account: The Exchange account.
        internal_reply: Auto-reply message for internal senders.
        external_reply: Auto-reply message for external senders (defaults to internal_reply).
        external_audience: Who gets external replies ('All', 'Known', or 'None').

    Returns:
        Updated OOF settings dictionary.
    """
    return set_oof_settings(
        account=account,
        state=OofSettings.ENABLED,
        external_audience=external_audience,
        internal_reply=internal_reply,
        external_reply=external_reply or internal_reply,
    )


def schedule_oof(
    account: Account,
    start: str,
    end: str,
    internal_reply: str,
    external_reply: Optional[str] = None,
    external_audience: str = "All",
) -> Dict[str, Any]:
    """Schedule Out-of-Office for a future period.

    Args:
        account: The Exchange account.
        start: ISO datetime string for scheduled start.
        end: ISO datetime string for scheduled end.
        internal_reply: Auto-reply message for internal senders.
        external_reply: Auto-reply message for external senders (defaults to internal_reply).
        external_audience: Who gets external replies ('All', 'Known', or 'None').

    Returns:
        Updated OOF settings dictionary.
    """
    return set_oof_settings(
        account=account,
        state=OofSettings.SCHEDULED,
        external_audience=external_audience,
        start=start,
        end=end,
        internal_reply=internal_reply,
        external_reply=external_reply or internal_reply,
    )


def disable_oof(account: Account) -> Dict[str, Any]:
    """Disable Out-of-Office.

    Args:
        account: The Exchange account.

    Returns:
        Updated OOF settings dictionary.
    """
    return set_oof_settings(
        account=account,
        state=OofSettings.DISABLED,
        internal_reply="",
        external_reply="",
    )
