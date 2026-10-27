from fastapi import FastAPI

app = FastAPI()


@app.get("/api/notes/v2")
def list_notes():
    return []
