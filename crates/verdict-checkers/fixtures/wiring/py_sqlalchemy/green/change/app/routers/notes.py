from fastapi import APIRouter, Depends
from sqlalchemy.orm import Session

from app.db import get_db
from app.models import Note
from app.services import save_note

router = APIRouter(prefix="/notes")


@router.get("")
def list_notes(db: Session = Depends(get_db)):
    return [n.text for n in db.query(Note).all()]


@router.post("")
def create_note(text: str, db: Session = Depends(get_db)):
    return save_note(db, text)
