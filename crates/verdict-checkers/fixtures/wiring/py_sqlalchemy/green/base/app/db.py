from sqlalchemy import create_engine
from sqlalchemy.orm import sessionmaker

engine = create_engine("sqlite:///notes.db")
SessionLocal = sessionmaker(engine)


def get_db():
    with SessionLocal() as db:
        yield db
