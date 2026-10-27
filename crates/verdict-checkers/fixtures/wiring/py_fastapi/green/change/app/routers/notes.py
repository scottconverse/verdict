from fastapi import APIRouter, Depends
from sqlalchemy.orm import Session

from app.db import get_db
from app.models import Note

router = APIRouter(prefix="/notes")


@router.post("")
def create_note(text: str, db: Session = Depends(get_db)):
    note = Note(text=text)
    db.add(note)
    db.commit()
    return {"id": note.id}
