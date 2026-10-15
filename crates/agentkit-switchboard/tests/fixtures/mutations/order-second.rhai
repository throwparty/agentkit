fn mutate(request) {
    request.headers["x-order"] = request.headers["x-order"] + "-second";
    request
}
