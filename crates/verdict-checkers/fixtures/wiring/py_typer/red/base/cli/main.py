import typer

app = typer.Typer()


@app.command()
def version():
    print("1.0")


if __name__ == "__main__":
    app()
