"""Tests for the mail module."""

import pytest
from unittest.mock import MagicMock, patch, PropertyMock
import os
import tempfile

# Mock exchangelib before importing mail module
import sys

sys.modules["exchangelib"] = MagicMock()
sys.modules["exchangelib.account"] = MagicMock()

from h8 import mail


class TestGetFolder:
    """Tests for get_folder function."""

    def test_get_inbox(self):
        """get_folder should return inbox for 'inbox'."""
        mock_account = MagicMock()
        mock_inbox = MagicMock()
        mock_account.inbox = mock_inbox

        result = mail.get_folder(mock_account, "inbox")
        assert result == mock_inbox

    def test_get_sent(self):
        """get_folder should return sent for 'sent'."""
        mock_account = MagicMock()
        mock_sent = MagicMock()
        mock_account.sent = mock_sent

        result = mail.get_folder(mock_account, "sent")
        assert result == mock_sent

    def test_get_drafts(self):
        """get_folder should return drafts for 'drafts'."""
        mock_account = MagicMock()
        mock_drafts = MagicMock()
        mock_account.drafts = mock_drafts

        result = mail.get_folder(mock_account, "drafts")
        assert result == mock_drafts

    def test_case_insensitive(self):
        """get_folder should be case-insensitive."""
        mock_account = MagicMock()
        mock_inbox = MagicMock()
        mock_account.inbox = mock_inbox

        result = mail.get_folder(mock_account, "INBOX")
        assert result == mock_inbox

    def test_custom_folder_not_found(self):
        """get_folder should raise ValueError for unknown folder."""
        mock_account = MagicMock()
        mock_account.root.walk.return_value = []

        with pytest.raises(ValueError, match="Folder 'unknown' not found"):
            mail.get_folder(mock_account, "unknown")


class TestListMessages:
    """Tests for list_messages function."""

    def test_list_messages_basic(self):
        """list_messages should return formatted message list."""
        mock_account = MagicMock()
        mock_folder = MagicMock()
        mock_account.inbox = mock_folder

        mock_item = MagicMock()
        mock_item.id = "msg-123"
        mock_item.changekey = "key-456"
        mock_item.subject = "Test Subject"
        mock_item.sender = MagicMock(email_address="sender@example.com")
        mock_item.to_recipients = [MagicMock(email_address="to@example.com")]
        mock_item.cc_recipients = []
        mock_item.datetime_received = MagicMock(isoformat=lambda: "2024-01-01T12:00:00")
        mock_item.is_read = True
        mock_item.has_attachments = False

        mock_query = MagicMock()
        mock_query.filter.return_value = mock_query
        mock_query.order_by.return_value = mock_query
        mock_query.only.return_value = mock_query
        mock_query.__getitem__ = lambda self, key: [mock_item]
        mock_folder.all.return_value = mock_query

        result = mail.list_messages(mock_account, "inbox", 20, False)

        assert len(result) == 1
        assert result[0]["id"] == "msg-123"
        assert result[0]["subject"] == "Test Subject"
        assert result[0]["from"] == "sender@example.com"

    def test_list_messages_unread_filter(self):
        """list_messages should filter by unread when specified."""
        mock_account = MagicMock()
        mock_folder = MagicMock()
        mock_account.inbox = mock_folder

        mock_query = MagicMock()
        mock_query.filter.return_value = mock_query
        mock_query.order_by.return_value = mock_query
        mock_query.only.return_value = mock_query
        mock_query.__getitem__ = lambda self, key: []
        mock_folder.all.return_value = mock_query

        mail.list_messages(mock_account, "inbox", 20, True)

        mock_query.filter.assert_called_once_with(is_read=False)


class TestFetchMessages:
    """Tests for fetch_messages function."""

    def test_fetch_to_maildir_creates_structure(self):
        """_fetch_to_maildir should create Maildir directory structure."""
        with tempfile.TemporaryDirectory() as tmpdir:
            mock_folder = MagicMock()
            mock_query = MagicMock()
            mock_query.order_by.return_value = mock_query
            mock_query.__getitem__ = lambda self, key: []
            mock_folder.all.return_value = mock_query

            result = mail._fetch_to_maildir(mock_folder, tmpdir, None)

            assert os.path.isdir(os.path.join(tmpdir, "cur"))
            assert os.path.isdir(os.path.join(tmpdir, "new"))
            assert os.path.isdir(os.path.join(tmpdir, "tmp"))
            assert result["success"] is True

    def test_fetch_unknown_format(self):
        """fetch_messages should return error for unknown format."""
        mock_account = MagicMock()
        mock_folder = MagicMock()
        mock_account.inbox = mock_folder

        result = mail.fetch_messages(mock_account, "inbox", "/tmp/test", "unknown")

        assert "error" in result
        assert "Unknown format" in result["error"]


class TestSendMessage:
    """Tests for send_message function."""

    def test_send_message_basic(self):
        """send_message should create and send message."""
        mock_account = MagicMock()
        mock_msg_class = MagicMock()
        mock_msg = MagicMock()
        mock_msg_class.return_value = mock_msg

        with patch.object(mail, "Message", mock_msg_class):
            result = mail.send_message(
                mock_account,
                {
                    "to": ["recipient@example.com"],
                    "subject": "Test Subject",
                    "body": "Test Body",
                },
            )

        mock_msg.send.assert_called_once()
        assert result["success"] is True
        assert result["subject"] == "Test Subject"


class TestDraftOperations:
    """Tests for draft management functions."""

    def test_save_draft(self):
        """save_draft should create and save a draft."""
        mock_account = MagicMock()
        mock_drafts = MagicMock()
        mock_account.drafts = mock_drafts

        mock_msg_class = MagicMock()
        mock_msg = MagicMock()
        mock_msg.id = "draft-123"
        mock_msg.changekey = "key-456"
        mock_msg_class.return_value = mock_msg

        with patch.object(mail, "Message", mock_msg_class):
            result = mail.save_draft(
                mock_account,
                {
                    "to": ["recipient@example.com"],
                    "subject": "Draft Subject",
                    "body": "Draft Body",
                },
            )

        mock_msg.save.assert_called_once()
        assert result["success"] is True
        assert result["id"] == "draft-123"

    def test_update_draft_not_found(self):
        """update_draft should return error when draft not found."""
        mock_account = MagicMock()
        mock_drafts = MagicMock()
        mock_drafts.all.return_value = []
        mock_account.drafts = mock_drafts

        result = mail.update_draft(
            mock_account, "nonexistent-id", {"subject": "New Subject"}
        )

        assert result["success"] is False
        assert "not found" in result["error"]

    def test_delete_draft_not_found(self):
        """delete_draft should return error when draft not found."""
        mock_account = MagicMock()
        mock_drafts = MagicMock()
        mock_drafts.all.return_value = []
        mock_account.drafts = mock_drafts

        result = mail.delete_draft(mock_account, "nonexistent-id")

        assert result["success"] is False
        assert "not found" in result["error"]

    def test_delete_draft_success(self):
        """delete_draft should delete the draft when found."""
        mock_account = MagicMock()
        mock_drafts = MagicMock()
        mock_draft = MagicMock()
        mock_draft.id = "draft-123"
        mock_drafts.all.return_value = [mock_draft]
        mock_account.drafts = mock_drafts

        result = mail.delete_draft(mock_account, "draft-123")

        mock_draft.delete.assert_called_once()
        assert result["success"] is True

    def test_list_drafts(self):
        """list_drafts should return formatted draft list."""
        mock_account = MagicMock()
        mock_drafts = MagicMock()

        mock_draft = MagicMock()
        mock_draft.id = "draft-123"
        mock_draft.changekey = "key-456"
        mock_draft.subject = "Draft Subject"
        mock_draft.to_recipients = [MagicMock(email_address="to@example.com")]
        mock_draft.cc_recipients = []
        mock_draft.last_modified_time = MagicMock(
            isoformat=lambda: "2024-01-01T12:00:00"
        )

        mock_query = MagicMock()
        mock_query.order_by.return_value = mock_query
        mock_query.only.return_value = mock_query
        mock_query.__getitem__ = lambda self, key: [mock_draft]
        mock_drafts.all.return_value = mock_query
        mock_account.drafts = mock_drafts

        result = mail.list_drafts(mock_account, 20)

        assert len(result) == 1
        assert result[0]["id"] == "draft-123"
        assert result[0]["subject"] == "Draft Subject"
