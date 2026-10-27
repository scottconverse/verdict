from sqlalchemy.orm import Session

from app.models import Note


def save_note(db: Session, text: str) -> dict:
    note = Note(text=text)
    db.add(note)
    db.commit()
    return {"id": note.id}
