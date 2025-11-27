"""Authentication and EWS account connection."""

import subprocess
from exchangelib import Account, Configuration, DELEGATE
from exchangelib import OAuth2AuthorizationCodeCredentials


def get_token(email: str) -> str:
    """Get OAuth2 access token from oama."""
    return subprocess.check_output(['oama', 'access', email]).decode().strip()


def get_account(email: str) -> Account:
    """Create and return an authenticated EWS Account."""
    token = get_token(email)
    
    credentials = OAuth2AuthorizationCodeCredentials(
        access_token={'access_token': token, 'token_type': 'Bearer'}
    )
    
    config = Configuration(
        server='outlook.office365.com',
        credentials=credentials,
    )
    
    return Account(
        primary_smtp_address=email,
        config=config,
        autodiscover=False,
        access_type=DELEGATE,
    )
