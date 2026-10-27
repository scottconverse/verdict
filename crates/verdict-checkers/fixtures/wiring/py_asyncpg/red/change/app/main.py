from fastapi import FastAPI

from app import events

app = FastAPI()
app.include_router(events.router, prefix="/api")


@app.get("/health")
def health():
    return {"ok": True}
