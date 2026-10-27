from sqlalchemy.orm import Session

from app.models import Note


def save_note(db: Session, text: str) -> dict:
    return {"text": text}
