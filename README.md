# myip
A simple command line tool to get the ip related information for your host.

## Usage
Currently `myip` only serves as a command line interface to [ipinfo](https://ipinfo.io) to retrieve the public ip related information.

> To get public ip

```bash
myip ip
```
```
12.166.238.132
```

> To get your current city

```bash
myip ip city
```
```
Chicago
```

> To get full information in JSON format

```bash
myip
```
```json
{
  "ip": "12.166.238.132",
  "city": "Chicago",
  "region": "Illinois",
  "country": "US",
  "loc": "41.8500,-87.6500",
  "org": "AS7018 AT&T Services, Inc.",
  "postal": "60666",
  "timezone": "America/Chicago",
  "readme": "https://ipinfo.io/missingauth"
}
```
