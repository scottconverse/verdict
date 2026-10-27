from fastapi import APIRouter

from app import db

router = APIRouter(prefix="/events")


async def insert_event(pool, kind: str) -> None:
    await pool.execute("INSERT INTO events (kind) VALUES ($1)", kind)


@router.post("")
async def create_event(kind: str):
    await insert_event(db.pool, kind)
    return {"ok": True}
