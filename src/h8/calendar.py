"""Calendar operations."""

import sys
import json
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo
from typing import Optional

from exchangelib import EWSDateTime, EWSTimeZone, CalendarItem
from exchangelib.account import Account


def list_events(
    account: Account,
    days: int = 7,
    from_date: Optional[str] = None,
    to_date: Optional[str] = None,
) -> list[dict]:
    """List calendar events."""
    tz = ZoneInfo('Europe/Berlin')
    
    if from_date:
        start_dt = datetime.fromisoformat(from_date).replace(tzinfo=tz)
    else:
        start_dt = datetime.now(tz=tz)
    
    if to_date:
        end_dt = datetime.fromisoformat(to_date).replace(tzinfo=tz)
    else:
        end_dt = start_dt + timedelta(days=days)
    
    start = EWSDateTime.from_datetime(start_dt)
    end = EWSDateTime.from_datetime(end_dt)
    
    events = []
    for item in account.calendar.view(start=start, end=end):
        if not hasattr(item, 'start'):
            continue
        
        # Handle all-day events (date vs datetime)
        start_str = item.start.isoformat() if hasattr(item.start, 'isoformat') else str(item.start)
        end_str = item.end.isoformat() if hasattr(item.end, 'isoformat') else str(item.end)
        
        events.append({
            'id': item.id,
            'changekey': item.changekey,
            'subject': item.subject,
            'start': start_str,
            'end': end_str,
            'location': item.location,
            'body': item.body if item.body else None,
            'organizer': item.organizer.email_address if item.organizer else None,
            'is_all_day': item.is_all_day,
            'is_cancelled': item.is_cancelled,
        })
    
    return events


def create_event(account: Account, event_data: dict) -> dict:
    """Create a calendar event from JSON data."""
    tz = EWSTimeZone.localzone()
    
    start_dt = datetime.fromisoformat(event_data['start'])
    end_dt = datetime.fromisoformat(event_data['end'])
    
    # Add timezone if not present
    if start_dt.tzinfo is None:
        start_dt = start_dt.replace(tzinfo=ZoneInfo('Europe/Berlin'))
    if end_dt.tzinfo is None:
        end_dt = end_dt.replace(tzinfo=ZoneInfo('Europe/Berlin'))
    
    item = CalendarItem(
        account=account,
        folder=account.calendar,
        subject=event_data['subject'],
        start=EWSDateTime.from_datetime(start_dt),
        end=EWSDateTime.from_datetime(end_dt),
        location=event_data.get('location'),
        body=event_data.get('body'),
    )
    item.save()
    
    return {
        'id': item.id,
        'changekey': item.changekey,
        'subject': item.subject,
        'start': item.start.isoformat(),
        'end': item.end.isoformat(),
    }


def delete_event(account: Account, item_id: str) -> dict:
    """Delete a calendar event by ID."""
    from exchangelib import ItemId
    
    # Find the item by ID
    items = list(account.calendar.filter(id=item_id))
    
    if not items:
        return {'success': False, 'error': 'Event not found'}
    
    items[0].delete()
    return {'success': True, 'id': item_id}
