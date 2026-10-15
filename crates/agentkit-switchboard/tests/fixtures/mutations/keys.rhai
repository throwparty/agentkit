fn mutate(request) {
    let seen = #{};
    for k in request.keys() {
        seen[k] = request[k];
    }
    request.body = #{ "keys": seen };
    request
}
