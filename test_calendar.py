import subprocess
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo
from exchangelib import Account, Configuration, DELEGATE, EWSDateTime, EWSTimeZone
from exchangelib import OAuth2AuthorizationCodeCredentials

# Get token from oama
token = subprocess.check_output(['oama', 'access', 'tommy.falkowski@iem.fraunhofer.de']).decode().strip()

credentials = OAuth2AuthorizationCodeCredentials(
    access_token={'access_token': token, 'token_type': 'Bearer'}
)

config = Configuration(
    server='outlook.office365.com',
    credentials=credentials,
)

account = Account(
    primary_smtp_address='tommy.falkowski@iem.fraunhofer.de',
    config=config,
    autodiscover=False,
    access_type=DELEGATE,
)

print("=== Calendar Events (next 7 days) ===")
tz = EWSTimeZone.localzone()
now = datetime.now(tz=ZoneInfo('Europe/Berlin'))
start = EWSDateTime.from_datetime(now)
end = EWSDateTime.from_datetime(now + timedelta(days=7))

try:
    # Don't use order_by() - view() already returns items chronologically
    # and order_by() can fail with mixed item types like _Booking
    for item in account.calendar.view(start=start, end=end)[:10]:
        # Skip items that don't have the expected attributes
        if not hasattr(item, 'start'):
            continue
        print(f"- {item.subject}")
        print(f"  Start: {item.start}")
        print(f"  End: {item.end}")
        print(f"  Location: {item.location}")
        print()
except Exception as e:
    import traceback
    traceback.print_exc()
