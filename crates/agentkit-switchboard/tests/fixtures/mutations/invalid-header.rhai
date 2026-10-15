fn mutate(request) {
    request.headers["bad header name!!"] = "value";
    request
}
