"""Find free slots in calendar."""

from datetime import datetime, timedelta, time
from zoneinfo import ZoneInfo
from typing import Optional, Any

from exchangelib import EWSDateTime
from exchangelib.account import Account

from .config import get_config


def find_free_slots(
    account: Account,
    weeks: int = 1,
    duration_minutes: int = 30,
    limit: Optional[int] = None,
    start_hour: Optional[int] = None,
    end_hour: Optional[int] = None,
    exclude_weekends: Optional[bool] = None,
) -> list[dict]:
    """Find free slots in the calendar.
    
    Uses the GetUserAvailability API for efficient free/busy queries.
    
    Args:
        account: EWS account
        weeks: Number of weeks to look at (1 = current week until Sunday)
        duration_minutes: Minimum duration of free slot in minutes
        limit: Maximum number of slots to return
        start_hour: Start of working hours (default from config)
        end_hour: End of working hours (default from config)
        exclude_weekends: Whether to exclude weekends (default from config)
    
    Returns:
        List of free slot dictionaries with start, end, duration_minutes
    """
    config = get_config()
    fs_config = config.get('free_slots', {})
    
    # Use config defaults if not specified
    work_start_hour = start_hour if start_hour is not None else fs_config.get('start_hour', 9)
    work_end_hour = end_hour if end_hour is not None else fs_config.get('end_hour', 17)
    skip_weekends = exclude_weekends if exclude_weekends is not None else fs_config.get('exclude_weekends', True)
    
    tz = ZoneInfo(config.get('timezone', 'Europe/Berlin'))
    now = datetime.now(tz=tz)
    
    # Calculate end of period (end of current week + additional weeks)
    # weekday(): Monday=0, Sunday=6
    days_until_sunday = 6 - now.weekday()
    end_of_week = now + timedelta(days=days_until_sunday)
    # Add additional weeks
    end_date = end_of_week + timedelta(weeks=weeks - 1)
    # Set to end of day
    end_date = end_date.replace(hour=23, minute=59, second=59)
    
    start = EWSDateTime.from_datetime(now)
    end = EWSDateTime.from_datetime(end_date)
    
    # Use GetUserAvailability API - much faster than calendar.view()
    # Returns calendar events with busy_type: Free, Tentative, Busy, OOF, WorkingElsewhere, NoData
    busy_times: list[tuple[datetime, datetime]] = []
    
    try:
        info = list(account.protocol.get_free_busy_info(
            accounts=[(account, 'Required', False)],
            start=start,
            end=end,
            merged_free_busy_interval=15,  # 15 minute granularity
            requested_view='FreeBusy'
        ))
        
        if info and len(info) > 0:
            fb_view: Any = info[0]
            if hasattr(fb_view, 'calendar_events') and fb_view.calendar_events:
                for event in fb_view.calendar_events:
                    # Skip free events (we only care about busy times)
                    if event.busy_type == 'Free':
                        continue
                    
                    # Convert to local timezone
                    event_start = event.start
                    event_end = event.end
                    
                    if hasattr(event_start, 'tzinfo') and event_start.tzinfo is not None:
                        event_start = event_start.astimezone(tz)
                    if hasattr(event_end, 'tzinfo') and event_end.tzinfo is not None:
                        event_end = event_end.astimezone(tz)
                    
                    busy_times.append((event_start, event_end))
    except Exception:
        # Fallback to calendar.view() if GetUserAvailability fails
        pass
    
    # Fallback if no busy times from GetUserAvailability
    if not busy_times:
        busy_times = _get_busy_times_from_calendar(account, start, end, tz)
    
    # Sort busy times
    busy_times.sort(key=lambda x: x[0])
    
    # Merge overlapping busy times
    merged_busy: list[tuple[datetime, datetime]] = []
    for busy_start, busy_end in busy_times:
        if merged_busy and busy_start <= merged_busy[-1][1]:
            # Overlapping, extend the last busy period
            merged_busy[-1] = (merged_busy[-1][0], max(merged_busy[-1][1], busy_end))
        else:
            merged_busy.append((busy_start, busy_end))
    
    # Find free slots
    return _find_slots_from_busy_times(
        merged_busy, now, end_date, tz,
        work_start_hour, work_end_hour, skip_weekends,
        duration_minutes, limit
    )


def _get_busy_times_from_calendar(
    account: Account,
    start: EWSDateTime,
    end: EWSDateTime,
    tz: ZoneInfo
) -> list[tuple[datetime, datetime]]:
    """Fallback: Get busy times from calendar.view() with .only() optimization."""
    busy_times: list[tuple[datetime, datetime]] = []
    
    calendar: Any = account.calendar
    query = calendar.view(start=start, end=end).only(
        'start', 'end', 'is_cancelled'
    )
    
    for item in query:
        if not hasattr(item, 'start') or not hasattr(item, 'end'):
            continue
        if hasattr(item, 'is_cancelled') and item.is_cancelled:
            continue
        
        item_start = item.start
        item_end = item.end
        
        # Handle all-day events (EWSDate has no hour attribute)
        if not hasattr(item_start, 'hour'):
            item_start = datetime.combine(item_start, time(0, 0), tzinfo=tz)
            item_end = datetime.combine(item_end, time(0, 0), tzinfo=tz)
        else:
            if hasattr(item_start, 'tzinfo') and item_start.tzinfo is not None:
                item_start = datetime(
                    item_start.year, item_start.month, item_start.day,
                    item_start.hour, item_start.minute, item_start.second,
                    tzinfo=item_start.tzinfo
                ).astimezone(tz)
            else:
                item_start = datetime(
                    item_start.year, item_start.month, item_start.day,
                    item_start.hour, item_start.minute, item_start.second,
                    tzinfo=tz
                )
            
            if hasattr(item_end, 'tzinfo') and item_end.tzinfo is not None:
                item_end = datetime(
                    item_end.year, item_end.month, item_end.day,
                    item_end.hour, item_end.minute, item_end.second,
                    tzinfo=item_end.tzinfo
                ).astimezone(tz)
            else:
                item_end = datetime(
                    item_end.year, item_end.month, item_end.day,
                    item_end.hour, item_end.minute, item_end.second,
                    tzinfo=tz
                )
        
        busy_times.append((item_start, item_end))
    
    return busy_times


def _find_slots_from_busy_times(
    merged_busy: list[tuple[datetime, datetime]],
    now: datetime,
    end_date: datetime,
    tz: ZoneInfo,
    start_hour: int,
    end_hour: int,
    exclude_weekends: bool,
    duration_minutes: int,
    limit: Optional[int],
) -> list[dict]:
    """Find free slots given merged busy times."""
    free_slots: list[dict] = []
    duration = timedelta(minutes=duration_minutes)
    
    current_day = now.date()
    end_day = end_date.date()
    
    while current_day <= end_day:
        # Skip weekends if configured
        if exclude_weekends and current_day.weekday() >= 5:
            current_day += timedelta(days=1)
            continue
        
        # Define working hours for this day
        day_start = datetime.combine(current_day, time(start_hour, 0), tzinfo=tz)
        day_end = datetime.combine(current_day, time(end_hour, 0), tzinfo=tz)
        
        # If it's today, start from now (but not before start_hour)
        if current_day == now.date():
            day_start = max(day_start, now)
            # Round up to next 15-minute slot
            minutes = day_start.minute
            if minutes % 15 != 0:
                day_start = day_start.replace(
                    minute=(minutes // 15 + 1) * 15 % 60,
                    second=0, microsecond=0
                )
                if minutes >= 45:
                    day_start += timedelta(hours=1)
                    day_start = day_start.replace(minute=0)
        
        # Find free slots in this day
        slot_start = day_start
        
        for busy_start, busy_end in merged_busy:
            # Skip busy times before our current slot
            if busy_end <= slot_start:
                continue
            # Stop if busy time is after this day
            if busy_start >= day_end:
                break
            
            # Check if there's a free slot before this busy time
            if busy_start > slot_start:
                free_end = min(busy_start, day_end)
                slot_duration = free_end - slot_start
                
                if slot_duration >= duration:
                    free_slots.append({
                        'start': slot_start.isoformat(),
                        'end': free_end.isoformat(),
                        'duration_minutes': int(slot_duration.total_seconds() / 60),
                        'date': current_day.isoformat(),
                        'day': current_day.strftime('%A'),
                    })
                    
                    if limit and len(free_slots) >= limit:
                        return free_slots
            
            # Move slot_start past this busy time
            slot_start = max(slot_start, busy_end)
        
        # Check for free slot after all busy times
        if slot_start < day_end:
            slot_duration = day_end - slot_start
            if slot_duration >= duration:
                free_slots.append({
                    'start': slot_start.isoformat(),
                    'end': day_end.isoformat(),
                    'duration_minutes': int(slot_duration.total_seconds() / 60),
                    'date': current_day.isoformat(),
                    'day': current_day.strftime('%A'),
                })
                
                if limit and len(free_slots) >= limit:
                    return free_slots
        
        current_day += timedelta(days=1)
    
    return free_slots
