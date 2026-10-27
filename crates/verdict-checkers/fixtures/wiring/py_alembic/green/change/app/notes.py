from sqlalchemy.orm import Session

from app.models import Note


def add_note(session: Session, text: str) -> None:
    session.add(Note(text=text))
    session.commit()
